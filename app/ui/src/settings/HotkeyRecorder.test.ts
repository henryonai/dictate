import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { setLanguage } from "../lib/i18n";
import HotkeyRecorder from "./HotkeyRecorder.svelte";

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

/** The recorder, with `onrecord` answering what the Rust side would ("" = the shortcut was taken). */
function renderRecorder(answer = "", props: { label?: string; canChange?: boolean; onlistening?: (active: boolean) => void } = {}) {
  const onrecord = vi.fn(async (_accelerator: string) => answer);
  render(HotkeyRecorder, { label: "Ctrl+Alt+Space", onrecord, ...props });
  return onrecord;
}

/** A key goes down on the window, as it does wherever the focus is. */
function keydown(code: string, held: Partial<KeyboardEventInit> = {}) {
  return fireEvent.keyDown(window, { code, ...held });
}

async function startListening() {
  await userEvent.setup().click(screen.getByRole("button", { name: "Change…" }));
}

test("shows the shortcut and a button to change it", () => {
  renderRecorder();

  expect(screen.getByLabelText("Shortcut")).toHaveValue("Ctrl+Alt+Space");
  expect(screen.getByLabelText("Shortcut")).toHaveAttribute("readonly");
  expect(screen.getByRole("button", { name: "Change…" })).toBeInTheDocument();
});

test("Change… starts listening and says what to press", async () => {
  renderRecorder();

  await startListening();

  expect(screen.getByLabelText("Shortcut")).toHaveValue("Press the new shortcut…");
  // Shift is not offered as enough, and "Cmd" is not said where there is none.
  expect(screen.getByText(/Esc cancels/)).toHaveTextContent(
    "Hold Ctrl, Alt or ⌘/Win and press a key (Shift can be added), or press a function key (F1–F24). Esc cancels.",
  );
  expect(screen.queryByRole("button", { name: "Change…" })).not.toBeInTheDocument();
});

test("pressing a shortcut hands it over and stops listening", async () => {
  const onrecord = renderRecorder();
  await startListening();

  await keydown("KeyD", { ctrlKey: true, altKey: true });

  expect(onrecord).toHaveBeenCalledExactlyOnceWith("Ctrl+Alt+D");
  expect(await screen.findByRole("button", { name: "Change…" })).toBeInTheDocument();
  expect(screen.getByLabelText("Shortcut")).toHaveValue("Ctrl+Alt+Space");
});

test("a function key is enough by itself", async () => {
  const onrecord = renderRecorder();
  await startListening();

  await keydown("F6");

  expect(onrecord).toHaveBeenCalledExactlyOnceWith("F6");
});

test("the Command key makes Super", async () => {
  const onrecord = renderRecorder();
  await startListening();

  await keydown("KeyD", { metaKey: true, shiftKey: true });

  expect(onrecord).toHaveBeenCalledExactlyOnceWith("Shift+Super+D");
});

test("a modifier on its own, and a letter with none or with Shift only, are not a shortcut: it goes on listening", async () => {
  const onrecord = renderRecorder();
  await startListening();

  await keydown("ControlLeft", { ctrlKey: true });
  await keydown("KeyD");
  await keydown("KeyD", { shiftKey: true });

  expect(onrecord).not.toHaveBeenCalled();
  expect(screen.getByLabelText("Shortcut")).toHaveValue("Press the new shortcut…");
});

test("Esc cancels", async () => {
  const onrecord = renderRecorder();
  await startListening();

  await keydown("Escape");

  expect(onrecord).not.toHaveBeenCalled();
  expect(screen.getByLabelText("Shortcut")).toHaveValue("Ctrl+Alt+Space");
  expect(screen.getByRole("button", { name: "Change…" })).toBeInTheDocument();
});

test("the Cancel button cancels", async () => {
  const user = userEvent.setup();
  const onrecord = renderRecorder();
  await startListening();

  await user.click(screen.getByRole("button", { name: "Cancel" }));
  await keydown("KeyD", { ctrlKey: true });

  expect(onrecord).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Change…" })).toBeInTheDocument();
});

test("the window losing the focus cancels, so that no key is taken while another app is in front", async () => {
  const onrecord = renderRecorder();
  await startListening();

  await fireEvent.blur(window);
  await keydown("KeyD", { ctrlKey: true });

  expect(onrecord).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Change…" })).toBeInTheDocument();
});

test("while listening the keys belong to the recorder, not to the page", async () => {
  renderRecorder();
  await startListening();

  // `fireEvent` answers false when something called `preventDefault`.
  expect(await keydown("Tab")).toBe(false);
  expect(await keydown("KeyW", { ctrlKey: true })).toBe(false);
});

test("without listening, keys are left alone", async () => {
  const onrecord = renderRecorder();

  expect(await keydown("KeyD", { ctrlKey: true })).toBe(true);
  expect(onrecord).not.toHaveBeenCalled();
});

test("a shortcut that another app owns shows the taken message", async () => {
  renderRecorder("hotkey.taken");
  await startListening();

  await keydown("Space", { ctrlKey: true, altKey: true });

  expect(await screen.findByRole("alert")).toHaveTextContent("That shortcut is already used by another app. Try a different one.");
});

test("any other failure shows its own words", async () => {
  renderRecorder("Could not save the settings: disk full");
  await startListening();

  await keydown("F6");

  expect(await screen.findByRole("alert")).toHaveTextContent("Could not save the settings: disk full");
});

test("the message goes when a new try begins", async () => {
  renderRecorder("hotkey.taken");
  await startListening();
  await keydown("F6");
  await screen.findByRole("alert");

  await startListening();

  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("the message is in the language of the window", async () => {
  setLanguage("vi");
  renderRecorder("hotkey.taken");
  await userEvent.setup().click(screen.getByRole("button", { name: "Đổi…" }));

  await keydown("F6");

  expect(await screen.findByRole("alert")).toHaveTextContent("Phím tắt này đang được app khác dùng. Hãy thử phím khác.");
});

test("a shortcut the desktop owns has no Change button", () => {
  renderRecorder("", { canChange: false });

  expect(screen.getByLabelText("Shortcut")).toHaveValue("Ctrl+Alt+Space");
  expect(screen.queryByRole("button", { name: "Change…" })).not.toBeInTheDocument();
});

test("tells when it starts and stops listening, so that the shortcut is paused meanwhile", async () => {
  const onlistening = vi.fn();
  renderRecorder("", { onlistening });
  expect(onlistening).not.toHaveBeenCalled();

  await startListening();
  expect(onlistening).toHaveBeenCalledExactlyOnceWith(true);

  await keydown("F6");
  await vi.waitFor(() => expect(onlistening).toHaveBeenLastCalledWith(false));
  expect(onlistening).toHaveBeenCalledTimes(2);
});

test.each([
  ["Esc", () => keydown("Escape")],
  ["the Cancel button", () => userEvent.setup().click(screen.getByRole("button", { name: "Cancel" }))],
  ["the window losing the focus", () => fireEvent.blur(window)],
])("%s ends the pause too", async (_how, end) => {
  const onlistening = vi.fn();
  renderRecorder("", { onlistening });
  await startListening();

  await end();

  await vi.waitFor(() => expect(onlistening).toHaveBeenLastCalledWith(false));
  expect(onlistening).toHaveBeenCalledTimes(2);
});

test("the window going away while it listens ends the pause", async () => {
  const onlistening = vi.fn();
  const { unmount } = render(HotkeyRecorder, { label: "F6", onrecord: vi.fn(), onlistening });
  await userEvent.setup().click(screen.getByRole("button", { name: "Change…" }));

  unmount();

  expect(onlistening).toHaveBeenLastCalledWith(false);
});
