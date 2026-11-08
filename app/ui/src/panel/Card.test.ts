import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakePanelState, stubPanelApi } from "../test-helpers";
import Card from "./Card.svelte";
import { languageName } from "./state";

beforeEach(() => {
  stubPanelApi();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  setLanguage("en");
});

const review = (overrides: Parameters<typeof fakePanelState>[0] = {}) =>
  fakePanelState({ view: "card", phase: "review", text: "Hello there.", durationS: 65, ...overrides });

/** Pretends to be a Mac, or any other system. */
function system(platform: "MacIntel" | "Win32") {
  vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
}

const box = () => screen.getByRole("textbox", { name: "The dictated text" }) as HTMLTextAreaElement;

/** A key pressed in the window, as the card hears it. */
const press = (key: string, code: string, held: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) =>
  fireEvent.keyDown(window, { key, code, ...held });

// ---- What it shows ------------------------------------------------------------------------------------------------

test("the header says where the text comes from, and the meta line how long it was and in what language", () => {
  render(Card, { panel: review({ style: "concise", language: "en-US", durationS: 65 }) });
  expect(screen.getByText("Edited — Shorten")).toBeInTheDocument();
  expect(screen.getByText(`01:05 · ${languageName("en-US", "en")}`)).toBeInTheDocument();
});

test("the dot is green, and purple while the AI is editing", () => {
  const { container, rerender } = render(Card, { panel: review() });
  expect(container.querySelector(".dot")).not.toHaveClass("refining");

  rerender({ panel: review({ phase: "refining", style: "concise" }) });
  expect(container.querySelector(".dot")).toHaveClass("refining");
  expect(screen.getByText("AI is editing: Shorten…")).toBeInTheDocument();
});

test("the box holds the text, which can be edited", async () => {
  render(Card, { panel: review({ text: "Hello there." }) });
  expect(box()).toHaveValue("Hello there.");
  expect(box()).not.toBeDisabled();

  await fireEvent.input(box(), { target: { value: "Hello, there!" } });
  expect(box()).toHaveValue("Hello, there!");
});

test("the box has the focus when the card opens", () => {
  render(Card, { panel: review() });
  expect(box()).toHaveFocus();
});

test("while the AI edits the box is disabled and a spinner says so, and the buttons wait", () => {
  render(Card, { panel: review({ phase: "refining", style: "concise" }) });
  expect(box()).toBeDisabled();
  expect(screen.getByRole("status")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Shorten" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Copy & close" })).toBeDisabled();
});

test("a notice is the error label of the card", () => {
  render(Card, { panel: review({ notice: { key: "edit_failed", detail: "rate limited" } }) });
  expect(screen.getByRole("alert")).toHaveTextContent("Couldn't edit: rate limited");
});

test("with no notice there is no error label", () => {
  render(Card, { panel: review() });
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("a text the controller sends takes the place of what is in the box, and the same text does not", async () => {
  const { rerender } = render(Card, { panel: review({ text: "first" }) });
  await fireEvent.input(box(), { target: { value: "first, and what I typed" } });

  // A state that carries the same text (a notice came, say) leaves what was typed alone.
  rerender({ panel: review({ text: "first", notice: { key: "edit_failed", detail: "x" } }) });
  expect(box()).toHaveValue("first, and what I typed");

  // The AI's answer is new text: it is what the box holds now.
  rerender({ panel: review({ text: "second" }) });
  expect(box()).toHaveValue("second");
});

// ---- The style buttons --------------------------------------------------------------------------------------------

test("the five buttons are the styles of the Swift app, with their shortcuts, and the current one is marked", () => {
  system("MacIntel");
  render(Card, { panel: review({ style: "email" }) });
  const buttons = ["Fix errors & punctuation", "Shorten", "Turn into an email", "Bullet points", "Translate to English"];
  buttons.forEach((name, i) => {
    const button = screen.getByRole("button", { name });
    expect(button).toHaveAttribute("aria-keyshortcuts", `Meta+${i + 1}`);
    expect(button).toHaveAttribute("aria-pressed", String(name === "Turn into an email"));
    expect(button).toHaveTextContent(`⌘${i + 1}`);
  });
});

test("the shortcuts are written with Ctrl on the other systems", () => {
  system("Win32");
  render(Card, { panel: review() });
  const button = screen.getByRole("button", { name: "Shorten" });
  expect(button).toHaveAttribute("aria-keyshortcuts", "Control+2");
  expect(button).toHaveTextContent("Ctrl+2");
});

test("a button writes the text again in its style, after telling the Rust side what is in the box", async () => {
  render(Card, { panel: review({ text: "Hello there." }) });
  await fireEvent.input(box(), { target: { value: "Hello there, friend." } });
  await fireEvent.click(screen.getByRole("button", { name: "Shorten" }));

  await vi.waitFor(() => expect(api.dictationRewrite).toHaveBeenCalledExactlyOnceWith("concise"));
  expect(api.dictationCardText).toHaveBeenCalledExactlyOnceWith("Hello there, friend.");
  const told = vi.mocked(api.dictationCardText).mock.invocationCallOrder[0];
  const asked = vi.mocked(api.dictationRewrite).mock.invocationCallOrder[0];
  expect(told).toBeLessThan(asked);
});

test("the buttons wait while the text is empty", async () => {
  render(Card, { panel: review({ text: "Hello" }) });
  await fireEvent.input(box(), { target: { value: "   " } });
  expect(screen.getByRole("button", { name: "Shorten" })).toBeDisabled();
  await fireEvent.input(box(), { target: { value: "Hi" } });
  expect(screen.getByRole("button", { name: "Shorten" })).toBeEnabled();
});

test("More has the other three styles", async () => {
  render(Card, { panel: review() });
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();

  await fireEvent.click(screen.getByRole("button", { name: "More" }));
  const menu = screen.getByRole("menu");
  const items = within(menu).getAllByRole("menuitemradio");
  expect(items.map((item) => item.textContent?.trim())).toEqual([
    "Rewrite formally",
    "Turn into an AI prompt",
    "Translate to Vietnamese",
  ]);

  await fireEvent.click(items[2]);
  await vi.waitFor(() => expect(api.dictationRewrite).toHaveBeenCalledExactlyOnceWith("vi"));
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
});

test("a style that was chosen from More is the current one", async () => {
  render(Card, { panel: review({ style: "formal" }) });
  await fireEvent.click(screen.getByRole("button", { name: "More" }));
  expect(screen.getByRole("menuitemradio", { name: "Rewrite formally" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getByRole("menuitemradio", { name: "Translate to Vietnamese" })).toHaveAttribute("aria-checked", "false");
});

// ---- The shortcuts ------------------------------------------------------------------------------------------------

test("card_shortcuts_1_to_5: Cmd and a digit run the style on a Mac", async () => {
  system("MacIntel");
  render(Card, { panel: review() });
  const styles = ["clean", "concise", "email", "bullets", "en"];
  for (const [i, style] of styles.entries()) {
    vi.mocked(api.dictationRewrite).mockClear();
    await press(String(i + 1), `Digit${i + 1}`, { metaKey: true });
    await vi.waitFor(() => expect(api.dictationRewrite).toHaveBeenCalledExactlyOnceWith(style));
  }
});

test("card_shortcuts_1_to_5: Ctrl and a digit do nothing on a Mac", async () => {
  system("MacIntel");
  render(Card, { panel: review() });
  await press("2", "Digit2", { ctrlKey: true });
  await Promise.resolve();
  expect(api.dictationRewrite).not.toHaveBeenCalled();
});

test("card_shortcuts_1_to_5: Ctrl and a digit run the style elsewhere, and Cmd does nothing", async () => {
  system("Win32");
  render(Card, { panel: review() });
  await press("3", "Digit3", { metaKey: true });
  await Promise.resolve();
  expect(api.dictationRewrite).not.toHaveBeenCalled();

  await press("3", "Digit3", { ctrlKey: true });
  await vi.waitFor(() => expect(api.dictationRewrite).toHaveBeenCalledExactlyOnceWith("email"));
});

test("a style shortcut waits while the AI edits and while the text is empty, as its button does", async () => {
  system("MacIntel");
  const { rerender } = render(Card, { panel: review({ phase: "refining", style: "concise" }) });
  await press("1", "Digit1", { metaKey: true });
  await Promise.resolve();
  expect(api.dictationRewrite).not.toHaveBeenCalled();

  rerender({ panel: review({ text: "" }) });
  await press("1", "Digit1", { metaKey: true });
  await Promise.resolve();
  expect(api.dictationRewrite).not.toHaveBeenCalled();
});

test("card_esc_discards: Esc drops the dictation", async () => {
  render(Card, { panel: review() });
  await press("Escape", "Escape");
  expect(api.dictationCancel).toHaveBeenCalledOnce();
});

test("card_esc_discards: so does Esc in the box, and the Discard button", async () => {
  render(Card, { panel: review() });
  await fireEvent.keyDown(box(), { key: "Escape", code: "Escape" });
  expect(api.dictationCancel).toHaveBeenCalledOnce();

  await fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect(api.dictationCancel).toHaveBeenCalledTimes(2);
});

test("Esc that ends a composition of an input method is not an Esc for the card", async () => {
  render(Card, { panel: review() });
  await fireEvent.keyDown(box(), { key: "Escape", code: "Escape", isComposing: true });
  expect(api.dictationCancel).not.toHaveBeenCalled();
});

test("Esc closes the More menu first, and the next Esc discards", async () => {
  render(Card, { panel: review() });
  await fireEvent.click(screen.getByRole("button", { name: "More" }));

  await press("Escape", "Escape");
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(api.dictationCancel).not.toHaveBeenCalled();

  await press("Escape", "Escape");
  expect(api.dictationCancel).toHaveBeenCalledOnce();
});

test("the keys are not heard once the card is gone", async () => {
  const { unmount } = render(Card, { panel: review() });
  unmount();
  await press("Escape", "Escape");
  expect(api.dictationCancel).not.toHaveBeenCalled();
});

// ---- Undo ---------------------------------------------------------------------------------------------------------

test("card_undo_only_with_original: no Undo button, and no shortcut, without an original", async () => {
  system("MacIntel");
  render(Card, { panel: review({ original: null }) });
  expect(screen.queryByRole("button", { name: "Undo" })).not.toBeInTheDocument();

  await press("z", "KeyZ", { metaKey: true, altKey: true });
  expect(api.dictationUndo).not.toHaveBeenCalled();
});

test("card_undo_only_with_original: with one, the button and Alt+Cmd+Z undo", async () => {
  system("MacIntel");
  render(Card, { panel: review({ original: "hello there" }) });
  await fireEvent.click(screen.getByRole("button", { name: "Undo" }));
  expect(api.dictationUndo).toHaveBeenCalledOnce();

  await press("Ω", "KeyZ", { metaKey: true, altKey: true });
  expect(api.dictationUndo).toHaveBeenCalledTimes(2);
});

test("card_undo_only_with_original: elsewhere it is Ctrl+Alt+Z", async () => {
  system("Win32");
  render(Card, { panel: review({ original: "hello there" }) });
  await press("z", "KeyZ", { metaKey: true, altKey: true });
  expect(api.dictationUndo).not.toHaveBeenCalled();
  await press("z", "KeyZ", { ctrlKey: true, altKey: true });
  expect(api.dictationUndo).toHaveBeenCalledOnce();
});

test("Undo waits while the AI edits", async () => {
  system("MacIntel");
  render(Card, { panel: review({ phase: "refining", original: "hello there", style: "concise" }) });
  expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
  await press("z", "KeyZ", { metaKey: true, altKey: true });
  expect(api.dictationUndo).not.toHaveBeenCalled();
});

// ---- The custom request -------------------------------------------------------------------------------------------

test("card_custom_request_runs_on_enter: the field opens with the button, and Enter runs what is in it", async () => {
  render(Card, { panel: review() });
  expect(screen.queryByPlaceholderText(/Your own request/)).not.toBeInTheDocument();

  await fireEvent.click(screen.getByRole("button", { name: "＋ Custom request…" }));
  const field = screen.getByPlaceholderText("Your own request, e.g. rewrite for my boss, polite tone");
  expect(field).toHaveFocus();

  await fireEvent.input(field, { target: { value: "rewrite for my boss" } });
  await fireEvent.keyDown(field, { key: "Enter", code: "Enter" });
  await vi.waitFor(() => expect(api.dictationCustom).toHaveBeenCalledExactlyOnceWith("rewrite for my boss"));
  // The text of the box is told first, as for a style.
  expect(api.dictationCardText).toHaveBeenCalledWith("Hello there.");
});

test("card_custom_request_runs_on_enter: the Edit button runs it too, and an empty request runs nothing", async () => {
  render(Card, { panel: review() });
  await fireEvent.click(screen.getByRole("button", { name: "＋ Custom request…" }));
  const run = screen.getByRole("button", { name: "Edit" });
  expect(run).toBeDisabled();

  const field = screen.getByPlaceholderText(/Your own request/);
  await fireEvent.keyDown(field, { key: "Enter", code: "Enter" });
  await Promise.resolve();
  expect(api.dictationCustom).not.toHaveBeenCalled();

  await fireEvent.input(field, { target: { value: "  polite  " } });
  expect(run).toBeEnabled();
  await fireEvent.click(run);
  await vi.waitFor(() => expect(api.dictationCustom).toHaveBeenCalledExactlyOnceWith("polite"));
});

test("card_custom_request_runs_on_enter: Return in the field is not a shortcut of the card", async () => {
  system("MacIntel");
  render(Card, { panel: review() });
  await fireEvent.click(screen.getByRole("button", { name: "＋ Custom request…" }));
  const field = screen.getByPlaceholderText(/Your own request/);
  await fireEvent.input(field, { target: { value: "polite" } });
  await fireEvent.keyDown(field, { key: "Enter", code: "Enter", isComposing: true });
  await Promise.resolve();
  expect(api.dictationCustom).not.toHaveBeenCalled();
  expect(api.dictationCopy).not.toHaveBeenCalled();
});

test("the button closes the field again", async () => {
  render(Card, { panel: review() });
  const toggle = screen.getByRole("button", { name: "＋ Custom request…" });
  await fireEvent.click(toggle);
  expect(toggle).toHaveAttribute("aria-expanded", "true");
  await fireEvent.click(toggle);
  expect(screen.queryByPlaceholderText(/Your own request/)).not.toBeInTheDocument();
});

test("opened from the chip's Custom request…, the card has the field open and ready", () => {
  render(Card, { panel: review({ fromChip: true, openCustom: true }) });
  expect(screen.getByPlaceholderText(/Your own request/)).toHaveFocus();
});

test("the request waits while the AI edits", async () => {
  render(Card, { panel: review({ phase: "refining", style: "concise", openCustom: true }) });
  expect(screen.getByRole("button", { name: "Edit" })).toBeDisabled();
});

// ---- The footer ---------------------------------------------------------------------------------------------------

test("the footer of a card from the dictation: Discard, and Copy & close that copies the box", async () => {
  render(Card, { panel: review({ fromChip: false }) });
  expect(screen.queryByRole("button", { name: "Paste into field" })).not.toBeInTheDocument();

  await fireEvent.input(box(), { target: { value: "Edited text." } });
  await fireEvent.click(screen.getByRole("button", { name: "Copy & close" }));
  expect(api.dictationCopy).toHaveBeenCalledExactlyOnceWith("Edited text.");
});

test("card_from_chip_footer: Copy and Paste into field, and neither Discard nor Copy & close", async () => {
  render(Card, { panel: review({ fromChip: true }) });
  expect(screen.queryByRole("button", { name: "Discard" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copy & close" })).not.toBeInTheDocument();

  await fireEvent.input(box(), { target: { value: "Edited text." } });
  await fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  expect(api.dictationCopy).toHaveBeenCalledExactlyOnceWith("Edited text.");

  await fireEvent.click(screen.getByRole("button", { name: "Paste into field" }));
  expect(api.dictationPasteCard).toHaveBeenCalledExactlyOnceWith("Edited text.");
});

test("card_from_chip_footer: Cmd+Enter pastes the edited text as fresh text", async () => {
  system("MacIntel");
  render(Card, { panel: review({ fromChip: true }) });
  await fireEvent.input(box(), { target: { value: "Edited text." } });
  await fireEvent.keyDown(box(), { key: "Enter", code: "Enter", metaKey: true });
  expect(api.dictationPasteCard).toHaveBeenCalledExactlyOnceWith("Edited text.");
  expect(api.dictationCopy).not.toHaveBeenCalled();
});

test("Cmd+Enter in a card from the dictation is Copy & close", async () => {
  system("MacIntel");
  render(Card, { panel: review() });
  await press("Enter", "Enter", { metaKey: true });
  expect(api.dictationCopy).toHaveBeenCalledExactlyOnceWith("Hello there.");
});

test("Ctrl+Enter is the one elsewhere", async () => {
  system("Win32");
  render(Card, { panel: review() });
  await press("Enter", "Enter", { metaKey: true });
  expect(api.dictationCopy).not.toHaveBeenCalled();
  await press("Enter", "Enter", { ctrlKey: true });
  expect(api.dictationCopy).toHaveBeenCalledOnce();
});

test("Return alone is a new line in the box and no shortcut", async () => {
  render(Card, { panel: review() });
  await fireEvent.keyDown(box(), { key: "Enter", code: "Enter" });
  expect(api.dictationCopy).not.toHaveBeenCalled();
  expect(api.dictationPasteCard).not.toHaveBeenCalled();
});

test("Paste into field waits when the dictation can't write there", async () => {
  render(Card, { panel: review({ fromChip: true, canWrite: false }) });
  expect(screen.getByRole("button", { name: "Paste into field" })).toBeDisabled();
});

test("the hint names the shortcuts of the system, and leaves undo out when there is nothing to undo", () => {
  system("MacIntel");
  const { rerender } = render(Card, { panel: review({ original: "hello" }) });
  expect(screen.getByText("⌘1–5 edit · ⌥⌘Z undo · Esc discard")).toBeInTheDocument();

  rerender({ panel: review({ original: null }) });
  expect(screen.getByText("⌘1–5 edit · Esc discard")).toBeInTheDocument();

  rerender({ panel: review({ original: "hello", fromChip: true }) });
  expect(screen.getByText("⌘1–5 edit · ⌥⌘Z undo · Esc close")).toBeInTheDocument();
});

test("the hint is written with Ctrl on the other systems", () => {
  system("Win32");
  render(Card, { panel: review({ original: "hello" }) });
  expect(screen.getByText("Ctrl+1–5 edit · Ctrl+Alt+Z undo · Esc discard")).toBeInTheDocument();
});

// ---- Accessibility ------------------------------------------------------------------------------------------------

test("card_accessibility_hint: the card asks for the permission when writing into other apps needs it", async () => {
  render(Card, { panel: review({ accessibilityMissing: true }) });
  expect(screen.getByText("Grant Accessibility access so Dictate can write into chat boxes.")).toBeInTheDocument();

  await fireEvent.click(screen.getByRole("button", { name: "Grant access…" }));
  expect(api.dictationGrantAccessibility).toHaveBeenCalledOnce();
});

test("card_accessibility_hint: and says nothing when it isn't needed", () => {
  render(Card, { panel: review({ accessibilityMissing: false }) });
  expect(screen.queryByText(/Grant Accessibility access/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Grant access…" })).not.toBeInTheDocument();
});

// ---- Words --------------------------------------------------------------------------------------------------------

test("every word of the card is in Vietnamese when the UI is", () => {
  setLanguage("vi");
  system("MacIntel");
  render(Card, { panel: review({ original: "hello", accessibilityMissing: true }) });
  expect(screen.getByText("Xong — sửa hoặc sao chép")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Rút gọn" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Khác" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "＋ Yêu cầu riêng…" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Hoàn tác" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Bỏ" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Sao chép & đóng" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Cấp quyền…" })).toBeInTheDocument();
  expect(screen.getByText("⌘1–5 sửa · ⌥⌘Z hoàn tác · Esc bỏ")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Văn bản vừa đọc" })).toBeInTheDocument();
});
