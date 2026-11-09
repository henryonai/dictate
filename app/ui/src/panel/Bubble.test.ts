import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakePanelState, stubPanelApi } from "../test-helpers";
import Bubble from "./Bubble.svelte";
import type { PanelState } from "./state";

beforeEach(() => {
  stubPanelApi();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  setLanguage("en");
});

test("bubble_hint_toggle_vs_hold: a toggle dictation says which key finishes it", () => {
  render(Bubble, { panel: fakePanelState({ pushToTalk: false, hotkey: "⌃⌥Space" }) });
  expect(screen.getByText("⌃⌥Space to finish")).toBeInTheDocument();
});

test("bubble_hint_toggle_vs_hold: a hold dictation says to release the key", () => {
  render(Bubble, { panel: fakePanelState({ pushToTalk: true, hotkey: "Ctrl+Alt+Space" }) });
  expect(screen.getByText("Release Ctrl+Alt+Space to finish")).toBeInTheDocument();
  expect(screen.queryByText(/to finish$/, { selector: "*:not(.hint)" })).not.toBeInTheDocument();
});

test("bubble_hint_toggle_vs_hold: the hints are in Vietnamese when the UI is", () => {
  setLanguage("vi");
  const { rerender } = render(Bubble, { panel: fakePanelState({ pushToTalk: false }) });
  expect(screen.getByText("⌃⌥Space để xong")).toBeInTheDocument();
  rerender({ panel: fakePanelState({ pushToTalk: true }) });
  expect(screen.getByText("Thả ⌃⌥Space để xong")).toBeInTheDocument();
});

test("bubble_hint_toggle_vs_hold: while the AI edits, the key writes the text as it is", () => {
  const working = { kind: "editing" as const, style: "concise", model: null, editing: false };
  render(Bubble, { panel: fakePanelState({ phase: "working", working, text: "hello" }) });
  expect(screen.getByText("⌃⌥Space: write it as is")).toBeInTheDocument();
});

test("bubble_hint_toggle_vs_hold: no hint while writing or rewriting, where the key does something else", () => {
  for (const kind of ["writing", "rewriting"] as const) {
    cleanup();
    const working = { kind, style: "email", model: null, editing: false };
    const { container } = render(Bubble, { panel: fakePanelState({ phase: "working", working, text: "hello" }) });
    expect(container.querySelector(".hint"), kind).toBeNull();
  }
});

test("bubble_hint_toggle_vs_hold: no hint while finishing", () => {
  const { container } = render(Bubble, { panel: fakePanelState({ phase: "finishing", committed: "hi" }) });
  expect(container.querySelector(".hint")).toBeNull();
});

test("bubble_partial_is_dimmed: what is committed is white and the guess for the rest is grey 0.6", () => {
  render(Bubble, { panel: fakePanelState({ committed: "Hello there", partial: "how are" }) });

  expect(screen.getByText("Hello there")).not.toHaveStyle({ color: "rgba(255, 255, 255, 0.6)" });
  expect(screen.getByText("how are")).toHaveStyle({ color: "rgba(255, 255, 255, 0.6)" });
});

test("bubble_partial_is_dimmed: the two are joined with a space, and with none for Japanese", () => {
  const { container, rerender } = render(Bubble, {
    panel: fakePanelState({ committed: "Hello", partial: "there" }),
  });
  expect(container.querySelector(".text")).toHaveTextContent("Hello there");
  expect(container.querySelector(".text")?.textContent).toBe("Hello there");

  rerender({ panel: fakePanelState({ language: "ja-JP", committed: "こんにちは", partial: "世界" }) });
  expect(container.querySelector(".text")?.textContent).toBe("こんにちは世界");
});

test("bubble_partial_is_dimmed: nothing to join when one side is empty", () => {
  const { container } = render(Bubble, { panel: fakePanelState({ committed: "", partial: "first words" }) });
  expect(container.querySelector(".text")?.textContent).toBe("first words");
});

test("bubble_working_titles: each kind of work has its title", () => {
  const cases: [string, PanelState["working"], string][] = [
    ["writing", { kind: "writing", style: null, model: null, editing: false }, "Writing…"],
    ["editing", { kind: "editing", style: "concise", model: null, editing: false }, "Editing: Shorten…"],
    ["rewriting", { kind: "rewriting", style: "email", model: null, editing: false }, "Rewriting: Turn into an email…"],
    [
      "retranscribing",
      { kind: "retranscribing", style: null, model: "whisper-1", editing: false },
      "Re-transcribing with whisper-1…",
    ],
    [
      "retranscribing and editing",
      { kind: "retranscribing", style: "clean", model: "gpt-4o-transcribe", editing: true },
      "Re-transcribing and editing with gpt-4o-transcribe…",
    ],
  ];
  for (const [name, working, title] of cases) {
    cleanup();
    render(Bubble, { panel: fakePanelState({ phase: "working", working, text: "hello" }) });
    expect(screen.getByText(title), name).toBeInTheDocument();
  }
});

test("bubble_working_titles: a custom request is its own title", () => {
  const working = { kind: "editing" as const, style: "make it sound polite", model: null, editing: false };
  render(Bubble, { panel: fakePanelState({ phase: "working", working, text: "hello" }) });
  expect(screen.getByText("Editing: make it sound polite…")).toBeInTheDocument();
});

test("bubble_working_titles: and in Vietnamese", () => {
  setLanguage("vi");
  const working = { kind: "editing" as const, style: "concise", model: null, editing: false };
  render(Bubble, { panel: fakePanelState({ phase: "working", working, text: "hello" }) });
  expect(screen.getByText("Đang sửa: Rút gọn…")).toBeInTheDocument();
});

test("bubble_working_titles: listening and finishing", () => {
  const { rerender } = render(Bubble, { panel: fakePanelState({ phase: "recording" }) });
  expect(screen.getByText("Listening")).toBeInTheDocument();
  rerender({ panel: fakePanelState({ phase: "finishing", committed: "hi" }) });
  expect(screen.getByText("Finishing…")).toBeInTheDocument();
});

test("the text of the work is what was heard, at 70% opacity, with nothing dimmed", () => {
  const working = { kind: "writing" as const, style: null, model: null, editing: false };
  const { container } = render(Bubble, {
    panel: fakePanelState({ phase: "working", working, text: "The final words", committed: "ignored", partial: "ignored" }),
  });
  const text = container.querySelector(".text");
  expect(text?.textContent).toBe("The final words");
  expect(text).toHaveStyle({ opacity: "0.7" });
});

test("recording shows the dot, the waveform and the language; finishing shows none of them", () => {
  const { container, rerender } = render(Bubble, { panel: fakePanelState({ level: 0.5 }) });
  expect(screen.getByTestId("dot")).toBeInTheDocument();
  expect(container.querySelector(".wave")).not.toBeNull();
  expect(screen.getByRole("button", { name: "Change recognition language" })).toHaveTextContent("EN");

  rerender({ panel: fakePanelState({ phase: "finishing" }) });
  expect(screen.queryByTestId("dot")).not.toBeInTheDocument();
  expect(container.querySelector(".wave")).toBeNull();
  expect(screen.queryByRole("button", { name: "Change recognition language" })).not.toBeInTheDocument();
  expect(screen.getByRole("status")).toBeInTheDocument();
});

test("the dot is dimmer while it is quiet", () => {
  const { rerender } = render(Bubble, { panel: fakePanelState({ level: 0.03 }) });
  expect(screen.getByTestId("dot")).toHaveStyle({ opacity: "0.55" });
  rerender({ panel: fakePanelState({ level: 0.04 }) });
  expect(screen.getByTestId("dot")).toHaveStyle({ opacity: "1" });
});

test("an empty bubble fits its header, and one with text is 400 wide", () => {
  const { container, rerender } = render(Bubble, { panel: fakePanelState() });
  expect(container.querySelector(".bubble")).toHaveClass("fit");
  expect(container.querySelector(".text")).toBeNull();

  rerender({ panel: fakePanelState({ committed: "Some words" }) });
  expect(container.querySelector(".bubble")).not.toHaveClass("fit");
});

test("the cross cancels the dictation", async () => {
  render(Bubble, { panel: fakePanelState() });
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(api.dictationCancel).toHaveBeenCalledOnce();
});

test("the cross is Huỷ in Vietnamese", () => {
  setLanguage("vi");
  render(Bubble, { panel: fakePanelState() });
  expect(screen.getByRole("button", { name: "Huỷ" })).toBeInTheDocument();
});

test("the text goes on scrolling to the end as it grows", async () => {
  const { container, rerender } = render(Bubble, { panel: fakePanelState({ committed: "a" }) });
  const text = container.querySelector<HTMLElement>(".scroll")!;
  // jsdom lays nothing out: the scroll height is what a browser would say for text taller than the box.
  Object.defineProperty(text, "scrollHeight", { configurable: true, value: 300 });

  rerender({ panel: fakePanelState({ committed: "a", partial: "b" }) });
  await vi.waitFor(() => expect(text.scrollTop).toBe(300));
});

test("what went wrong while listening is said under the words, and the recording goes on", () => {
  render(Bubble, {
    panel: fakePanelState({ committed: "Hello", error: { key: "audio.silent", detail: null, action: null } }),
  });
  expect(screen.getByRole("alert")).toHaveTextContent("Dictate only hears silence");
  expect(screen.getByText("Listening")).toBeInTheDocument();
});

test("with nothing wrong there is no alert", () => {
  render(Bubble, { panel: fakePanelState({ committed: "Hello" }) });
  expect(screen.queryByRole("alert")).toBeNull();
});
