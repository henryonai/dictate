import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { RecordingView } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakeRecording, IDLE_RECORDING, stubRecordingApi } from "../test-helpers";
import RecorderApp from "./RecorderApp.svelte";

let events: ReturnType<typeof stubRecordingApi>;

beforeEach(() => {
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

/** Opens the bar as a window that was made in the middle of a recording does: it asks how the recording is, and hears the rest. */
async function open(view: RecordingView = fakeRecording({ phase: "recording", elapsedMs: 5_000 }), language: "en" | "vi" = "en") {
  events = stubRecordingApi();
  vi.mocked(api.recordingState).mockResolvedValue(view);
  vi.spyOn(api, "panelLanguage").mockResolvedValue(language);
  const rendered = render(RecorderApp);
  if (view.phase !== "idle") await screen.findByRole("img", { name: /^(Recording|Paused|Đang quay|Đã tạm dừng)$/ });
  return rendered;
}

const time = () => document.querySelector(".time")?.textContent;
const meter = (name: string) => screen.queryByRole("img", { name });
const litBars = (element: HTMLElement) => element.querySelectorAll(".bar.lit").length;

test("the bar asks how the recording is when it opens, and shows the dot, the time and the three buttons", async () => {
  await open();

  expect(api.recordingState).toHaveBeenCalledOnce();
  expect(screen.getByRole("img", { name: "Recording" })).toBeInTheDocument();
  expect(time()).toBe("00:05");
  expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Stop & summarize" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Discard recording" })).toBeEnabled();
});

test("the_timer_formats_hours: mm:ss, and h:mm:ss from an hour on, as the state comes", async () => {
  await open(fakeRecording({ phase: "recording", elapsedMs: 59_900 }));
  expect(time()).toBe("00:59");

  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 600_000 }));
  await vi.waitFor(() => expect(time()).toBe("10:00"));
  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 3_725_000 }));
  await vi.waitFor(() => expect(time()).toBe("1:02:05"));
});

test("a state that comes by the event while the answer is on its way is newer than the answer", async () => {
  events = stubRecordingApi();
  let answer: (view: RecordingView) => void = () => {};
  vi.mocked(api.recordingState).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
  vi.spyOn(api, "panelLanguage").mockResolvedValue("en");
  render(RecorderApp);
  await vi.waitFor(() => expect(api.recordingState).toHaveBeenCalled());

  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 9_000 }));
  await vi.waitFor(() => expect(time()).toBe("00:09"));
  answer(fakeRecording({ phase: "recording", elapsedMs: 1_000 }));
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(time()).toBe("00:09");
});

test("an idle recorder shows nothing: the window is hidden, and what it holds is not a bar with buttons", async () => {
  await open(IDLE_RECORDING);
  await vi.waitFor(() => expect(api.recordingState).toHaveBeenCalled());

  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  expect(time()).toBeUndefined();
});

test("meters_show_only_enabled_tracks: the one for the microphone and the one for the system each when its track is on", async () => {
  await open(fakeRecording({ phase: "recording", levels: { mic: 0.5, system: null } }));
  expect(meter("Mic: picking up sound")).toBeInTheDocument();
  expect(screen.queryByRole("img", { name: /^System/ })).not.toBeInTheDocument();

  events.sendRecordingState(fakeRecording({ phase: "recording", levels: { mic: null, system: 0 } }));
  await vi.waitFor(() => expect(meter("System: no sound yet")).toBeInTheDocument());
  expect(screen.queryByRole("img", { name: /^Mic/ })).not.toBeInTheDocument();

  events.sendRecordingState(fakeRecording({ phase: "recording", levels: { mic: 0.4, system: 0.02 } }));
  await vi.waitFor(() => expect(meter("Mic: picking up sound")).toBeInTheDocument());
  expect(meter("System: no sound yet")).toBeInTheDocument();
});

test("a meter has six bars, each lit while its share of the level is below it, and the last one is orange", async () => {
  await open(fakeRecording({ phase: "recording", levels: { mic: 0.5, system: 1 } }));

  const mic = meter("Mic: picking up sound")!;
  const system = meter("System: picking up sound")!;
  expect(mic.querySelectorAll(".bar")).toHaveLength(6);
  expect(litBars(mic)).toBe(3);
  expect(litBars(system)).toBe(6);
  expect(system.querySelector(".bar.last")).toHaveClass("lit");
  expect(mic.querySelectorAll(".bar.last")).toHaveLength(1);
  // The bars grow from 6 points by 2.
  expect([...mic.querySelectorAll<HTMLElement>(".bar")].map((bar) => bar.style.height)).toEqual(["6px", "8px", "10px", "12px", "14px", "16px"]);
});

test("a meter says whether the track picks up sound: above 0.05 it does", async () => {
  await open(fakeRecording({ phase: "recording", levels: { mic: 0.05, system: 0.06 } }));

  expect(meter("Mic: no sound yet")).toHaveAttribute("title", "Mic: no sound yet");
  expect(meter("System: picking up sound")).toHaveAttribute("title", "System: picking up sound");
});

test("pause_and_resume_call_the_commands: Pause while it records, and Resume while it is paused", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(screen.getByRole("button", { name: "Pause" }));
  expect(api.recordingPause).toHaveBeenCalledOnce();
  expect(api.recordingResume).not.toHaveBeenCalled();

  events.sendRecordingState(fakeRecording({ phase: "paused", elapsedMs: 7_000 }));
  const resume = await screen.findByRole("button", { name: "Resume recording" });
  expect(screen.getByRole("img", { name: "Paused" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();

  await user.click(resume);
  expect(api.recordingResume).toHaveBeenCalledOnce();
  expect(api.recordingPause).toHaveBeenCalledOnce();
});

test("Stop & summarize stops the recording, with the shortcut in its tooltip", async () => {
  const user = userEvent.setup();
  await open();
  const stop = screen.getByRole("button", { name: "Stop & summarize" });
  expect(stop).toHaveAttribute("title", "Stop recording and summarize (Ctrl+Alt+R)");

  await user.click(stop);

  expect(api.recordingStop).toHaveBeenCalledOnce();
});

test("on a Mac the tooltip of Stop writes the shortcut with the symbols of the keys", async () => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
  await open();

  expect(screen.getByRole("button", { name: "Stop & summarize" })).toHaveAttribute("title", "Stop recording and summarize (⌃⌥R)");
});

test("a stop that fails is left to the notification of the app: the bar goes on and can try again", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.recordingStop).mockRejectedValueOnce({ kind: "Other", key: "recording.error.stop_failed", detail: "disk full", command: null });

  await user.click(screen.getByRole("button", { name: "Stop & summarize" }));
  await vi.waitFor(() => expect(screen.getByRole("button", { name: "Stop & summarize" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "Stop & summarize" }));

  expect(api.recordingStop).toHaveBeenCalledTimes(2);
});

test("discard_asks_first_and_keep_recording_cancels", async () => {
  const user = userEvent.setup();
  await open();

  // The first click asks: nothing is thrown away, and the recording goes on under the question.
  await user.click(screen.getByRole("button", { name: "Discard recording" }));
  expect(screen.getByText("Discard this recording?")).toBeInTheDocument();
  expect(api.recordingDiscard).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Discard" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Stop & summarize" })).not.toBeInTheDocument();
  expect(time()).toBe("00:05");
  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 6_000 }));
  await vi.waitFor(() => expect(time()).toBe("00:06"));

  // Keep recording: the capsule is back, and nothing was discarded.
  await user.click(screen.getByRole("button", { name: "Keep recording" }));
  expect(screen.queryByText("Discard this recording?")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Stop & summarize" })).toBeInTheDocument();
  expect(api.recordingDiscard).not.toHaveBeenCalled();

  // Asked again and answered: it is thrown away, once.
  await user.click(screen.getByRole("button", { name: "Discard recording" }));
  await user.click(screen.getByRole("button", { name: "Discard" }));
  expect(api.recordingDiscard).toHaveBeenCalledOnce();
});

test("Esc is Keep recording", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Discard recording" }));

  await user.keyboard("{Escape}");

  expect(screen.queryByText("Discard this recording?")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Stop & summarize" })).toBeInTheDocument();
  expect(api.recordingDiscard).not.toHaveBeenCalled();
});

test("a recording that ends by itself under the question takes the question away: there is nothing left to discard", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Discard recording" }));

  events.sendRecordingState(fakeRecording({ phase: "finishing", elapsedMs: 8_000 }));

  await vi.waitFor(() => expect(screen.queryByText("Discard this recording?")).not.toBeInTheDocument());
  expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled();
});

test("a question that was left open is not open at the next recording", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Discard recording" }));
  events.sendRecordingState(IDLE_RECORDING);
  await vi.waitFor(() => expect(screen.queryByRole("button")).not.toBeInTheDocument());

  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 100 }));

  expect(await screen.findByRole("button", { name: "Stop & summarize" })).toBeInTheDocument();
  expect(screen.queryByText("Discard this recording?")).not.toBeInTheDocument();
});

test("finishing_disables_the_buttons: while it is put together, nothing can be pressed and the bar says it is saving", async () => {
  await open(fakeRecording({ phase: "finishing", elapsedMs: 31_000 }));

  expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Pause" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Discard recording" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Stop & summarize" })).not.toBeInTheDocument();
  expect(time()).toBe("00:31");
});

test("a recording that is being started is a recording that goes on", async () => {
  await open(fakeRecording({ phase: "starting", elapsedMs: 0 }));

  expect(time()).toBe("00:00");
  expect(screen.getByRole("img", { name: "Recording" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Stop & summarize" })).toBeInTheDocument();
});

test("a track that hears nothing, one that was reconnected and one that is off are each told, the one that is off most plainly", async () => {
  await open(
    fakeRecording({
      phase: "recording",
      levels: { mic: 0, system: null },
      notes: [
        { track: "mic", note: { kind: "silent" } },
        { track: "system", note: { kind: "off", detail: "the device is gone" } },
      ],
    }),
  );

  const silent = screen.getByText("Mic: silent");
  expect(silent).toHaveAttribute("title", "Nothing but silence has come from Mic so far.");
  const off = screen.getByText("System is off");
  expect(off).toHaveAttribute("title", "The device of System was lost and could not be taken up again: System is off for the rest of this recording.");
  expect(off).toHaveClass("off");
  expect(silent).not.toHaveClass("off");
  // The track that is off has no meter: it is not recording.
  expect(screen.queryByRole("img", { name: /^System/ })).not.toBeInTheDocument();
  expect(meter("Mic: no sound yet")).toBeInTheDocument();

  events.sendRecordingState(
    fakeRecording({
      phase: "recording",
      levels: { mic: 0.3, system: 0.3 },
      notes: [{ track: "system", note: { kind: "restarted", detail: "headphones plugged in" } }],
    }),
  );
  expect(await screen.findByText("System: reconnected")).toHaveAttribute(
    "title",
    "The device of System was lost and the recording went on with the one the system has now. What came between is silence.",
  );
  expect(screen.queryByText("Mic: silent")).not.toBeInTheDocument();
  expect(screen.queryByText("System is off")).not.toBeInTheDocument();
});

test("the notes are on the capsule itself, before the meters and outside the group that gives way when the room is short", async () => {
  await open(
    fakeRecording({
      phase: "recording",
      levels: { mic: 0, system: 0.3 },
      notes: [
        { track: "mic", note: { kind: "silent" } },
        { track: "system", note: { kind: "restarted", detail: "headphones" } },
      ],
    }),
  );

  const capsule = document.querySelector<HTMLElement>(".capsule")!;
  const meters = document.querySelector<HTMLElement>(".meters")!;
  const chips = [...capsule.querySelectorAll<HTMLElement>(".note")];
  expect(chips.map((chip) => chip.textContent?.trim())).toEqual(["System: reconnected", "Mic: silent"]);
  for (const chip of chips) {
    expect(chip.parentElement).toBe(capsule);
    expect(meters.contains(chip)).toBe(false);
    expect(chip.compareDocumentPosition(meters) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  }
});

test("a meter has no name written next to it while a note says what is the matter, or while there are two of them", async () => {
  await open(fakeRecording({ phase: "recording", levels: { mic: 0.2, system: null } }));
  expect(document.querySelector(".meters")).not.toHaveClass("bare");

  events.sendRecordingState(fakeRecording({ phase: "recording", levels: { mic: 0.2, system: 0.2 } }));
  await vi.waitFor(() => expect(document.querySelector(".meters")).toHaveClass("bare"));

  events.sendRecordingState(
    fakeRecording({ phase: "recording", levels: { mic: 0.2, system: null }, notes: [{ track: "system", note: { kind: "off", detail: "x" } }] }),
  );
  await vi.waitFor(() => expect(document.querySelector(".note.off")).toBeInTheDocument());
  expect(document.querySelector(".meters")).toHaveClass("bare");
});

test("with no track on there is no group of meters to take room", async () => {
  await open(
    fakeRecording({
      phase: "recording",
      mode: { kind: "audio" },
      levels: { mic: null, system: null },
      notes: [{ track: "mic", note: { kind: "off", detail: "unplugged" } }],
    }),
  );

  expect(document.querySelector(".meters")).toBeNull();
  expect(screen.getByText("Mic is off")).toBeInTheDocument();
});

test("a recording of sound alone whose only source is off says so, and the bar is still there to stop it", async () => {
  await open(
    fakeRecording({
      phase: "recording",
      mode: { kind: "audio" },
      levels: { mic: null, system: null },
      notes: [{ track: "mic", note: { kind: "off", detail: "unplugged" } }],
    }),
  );

  expect(screen.getByText("Mic is off")).toHaveClass("off");
  expect(screen.getByRole("button", { name: "Stop & summarize" })).toBeEnabled();
});

test("the capsule is where the bar is dragged from, and the buttons are not", async () => {
  await open();

  const capsule = document.querySelector(".capsule");
  expect(capsule).toHaveAttribute("data-tauri-drag-region");
  for (const button of screen.getAllByRole("button")) {
    expect(button).not.toHaveAttribute("data-tauri-drag-region");
  }
  // What is inside the capsule that is not a button (the time, the dot) is part of the handle, as the background is.
  expect(document.querySelector(".time")).toHaveAttribute("data-tauri-drag-region");
});

test("the bar speaks the language of the settings, which it asks for when the recording begins", async () => {
  events = stubRecordingApi();
  vi.spyOn(api, "panelLanguage").mockResolvedValue("en");
  render(RecorderApp);
  await vi.waitFor(() => expect(api.recordingState).toHaveBeenCalled());
  expect(api.panelLanguage).not.toHaveBeenCalled();

  vi.mocked(api.panelLanguage).mockResolvedValue("vi");
  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 0 }));

  expect(await screen.findByRole("button", { name: "Dừng & tóm tắt" })).toBeInTheDocument();
  expect(api.panelLanguage).toHaveBeenCalledOnce();
});

test("the bar is not drawn in the language of the time before: it waits for the answer", async () => {
  events = stubRecordingApi();
  let answer: (language: "en" | "vi") => void = () => {};
  vi.spyOn(api, "panelLanguage").mockImplementation(() => new Promise((resolve) => (answer = resolve)));
  render(RecorderApp);
  await vi.waitFor(() => expect(api.recordingState).toHaveBeenCalled());

  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 0 }));
  await vi.waitFor(() => expect(api.panelLanguage).toHaveBeenCalled());
  expect(screen.queryByRole("button")).not.toBeInTheDocument();

  answer("vi");
  expect(await screen.findByRole("button", { name: "Tạm dừng" })).toBeInTheDocument();
});

test("a language that can't be read leaves the bar in the language it has, and shows it all the same", async () => {
  events = stubRecordingApi();
  vi.spyOn(api, "panelLanguage").mockRejectedValue("The settings could not be read.");
  render(RecorderApp);
  await vi.waitFor(() => expect(api.recordingState).toHaveBeenCalled());

  events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 0 }));

  expect(await screen.findByRole("button", { name: "Pause" })).toBeInTheDocument();
});

test("vietnamese: the dot, the meters, the buttons and the question", async () => {
  const user = userEvent.setup();
  await open(fakeRecording({ phase: "recording", levels: { mic: 0.5, system: 0 } }), "vi");

  expect(screen.getByRole("img", { name: "Đang quay" })).toBeInTheDocument();
  expect(meter("Mic: có tiếng")).toBeInTheDocument();
  expect(meter("Hệ thống: chưa nghe thấy gì")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Tạm dừng" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Dừng & tóm tắt" })).toHaveAttribute("title", "Dừng quay và tóm tắt (Ctrl+Alt+R)");
  await user.click(screen.getByRole("button", { name: "Bỏ bản ghi" }));
  expect(screen.getByText("Bỏ bản ghi này?")).toBeInTheDocument();
  expect(within(document.body).getByRole("button", { name: "Bỏ" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Tiếp tục quay" })).toBeInTheDocument();
});

test("vietnamese: paused, saving and the notes", async () => {
  await open(
    fakeRecording({ phase: "paused", levels: { mic: 0, system: null }, notes: [{ track: "mic", note: { kind: "silent" } }] }),
    "vi",
  );
  expect(screen.getByRole("img", { name: "Đã tạm dừng" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Tiếp tục quay" })).toBeInTheDocument();
  expect(screen.getByText("Mic: im lặng")).toBeInTheDocument();

  events.sendRecordingState(fakeRecording({ phase: "finishing", levels: { mic: null, system: null } }));
  expect(await screen.findByRole("button", { name: "Đang lưu…" })).toBeDisabled();
});

test("the bar hears the recording until it goes away", async () => {
  const view = await open();
  expect(events.recordingListeners().state).toBe(1);

  view.unmount();

  await vi.waitFor(() => expect(events.recordingListeners().state).toBe(0));
});
