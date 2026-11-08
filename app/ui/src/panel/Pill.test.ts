import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakePanelState, stubPanelApi } from "../test-helpers";
import Pill from "./Pill.svelte";

beforeEach(() => {
  stubPanelApi();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  setLanguage("en");
});

const pillState = (overrides: Parameters<typeof fakePanelState>[0] = {}) =>
  fakePanelState({ view: "pill", durationS: 65, ...overrides });

test("pill_timer_and_caption: the timer shows the time as mm:ss", () => {
  render(Pill, { panel: pillState({ durationS: 65.4 }) });
  expect(screen.getByText("01:05")).toBeInTheDocument();
});

test("pill_timer_and_caption: the timer follows the state", () => {
  const { rerender } = render(Pill, { panel: pillState({ durationS: 9 }) });
  expect(screen.getByText("00:09")).toBeInTheDocument();
  rerender({ panel: pillState({ durationS: 3725 }) });
  expect(screen.getByText("1:02:05")).toBeInTheDocument();
});

test("pill_timer_and_caption: a toggle dictation is stopped by the key, Return or the button", () => {
  render(Pill, { panel: pillState({ pushToTalk: false, hotkey: "⌃⌥Space" }) });
  expect(screen.getByText("⌃⌥Space or ↩ to stop · Esc to cancel")).toBeInTheDocument();
});

test("pill_timer_and_caption: a hold dictation is stopped by releasing the key", () => {
  render(Pill, { panel: pillState({ pushToTalk: true, hotkey: "Ctrl+Alt+Space" }) });
  expect(screen.getByText("Release Ctrl+Alt+Space to stop · Esc to cancel")).toBeInTheDocument();
});

test("pill_timer_and_caption: the caption is in Vietnamese when the UI is", () => {
  setLanguage("vi");
  const { rerender } = render(Pill, { panel: pillState({ pushToTalk: false }) });
  expect(screen.getByText("⌃⌥Space hoặc ↩ để dừng · Esc huỷ")).toBeInTheDocument();
  rerender({ panel: pillState({ pushToTalk: true }) });
  expect(screen.getByText("Thả ⌃⌥Space để dừng · Esc huỷ")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Dừng" })).toBeInTheDocument();
});

test("pill_timer_and_caption: finishing says so on the button, which is disabled", () => {
  render(Pill, { panel: pillState({ phase: "finishing" }) });
  const button = screen.getByRole("button", { name: "Finishing…" });
  expect(button).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
});

test("the Stop button stops the dictation", async () => {
  render(Pill, { panel: pillState() });
  await fireEvent.click(screen.getByRole("button", { name: "Stop" }));
  expect(api.dictationStop).toHaveBeenCalledOnce();
});

test("Return stops the dictation, and Esc cancels it", async () => {
  render(Pill, { panel: pillState() });

  await fireEvent.keyDown(window, { key: "Enter" });
  expect(api.dictationStop).toHaveBeenCalledOnce();

  await fireEvent.keyDown(window, { key: "Escape" });
  expect(api.dictationCancel).toHaveBeenCalledOnce();
});

test("Return does not stop a recording that is already being finished", async () => {
  render(Pill, { panel: pillState({ phase: "finishing" }) });
  await fireEvent.keyDown(window, { key: "Enter" });
  expect(api.dictationStop).not.toHaveBeenCalled();
});

test("the keys are not heard once the pill is gone", async () => {
  const { unmount } = render(Pill, { panel: pillState() });
  unmount();
  await fireEvent.keyDown(window, { key: "Escape" });
  expect(api.dictationCancel).not.toHaveBeenCalled();
});

test("the live text is above the capsule, with the guess for the rest in grey 0.65", () => {
  const { container } = render(Pill, { panel: pillState({ committed: "Hello there", partial: "how are" }) });
  expect(container.querySelector(".live")).toHaveTextContent("Hello there how are");
  expect(screen.getByText("how are")).toHaveStyle({ color: "rgba(255, 255, 255, 0.65)" });
  expect(screen.getByText("Hello there")).not.toHaveStyle({ color: "rgba(255, 255, 255, 0.65)" });
});

test("with no words yet there is no box for them", () => {
  const { container } = render(Pill, { panel: pillState() });
  expect(container.querySelector(".live")).toBeNull();
});

test("the capsule has the dot, the waveform and the language of the dictation", () => {
  const { container } = render(Pill, { panel: pillState({ language: "vi-VN", level: 0.01 }) });
  expect(screen.getByTestId("dot")).toHaveStyle({ opacity: "0.55" });
  expect(container.querySelector(".wave")).not.toBeNull();
  expect(screen.getByRole("button", { name: "Change recognition language" })).toHaveTextContent("VI");
});
