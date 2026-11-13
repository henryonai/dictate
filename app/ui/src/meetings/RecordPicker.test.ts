import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { ErrorView, SourcesView } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeError, fakeSources, stubRecordingApi } from "../test-helpers";
import RecordPicker from "./RecordPicker.svelte";

let store: MeetingsStore;

beforeEach(() => {
  vi.spyOn(api, "copyText").mockResolvedValue();
  vi.spyOn(api, "openPermissionSettings").mockResolvedValue();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

/** Opens the picker over what `sources` says there is to record, and waits until it has been offered. */
async function open(sources: SourcesView = fakeSources(), error: ErrorView | null = null) {
  stubRecordingApi(sources);
  store = new MeetingsStore();
  store.openRecorder(error);
  const view = render(RecordPicker, { store });
  await screen.findAllByRole("radiogroup");
  return view;
}

const mode = (name: string) => screen.getByRole("radio", { name });
const start = () => screen.getByRole("button", { name: "Start recording" });

test("it is a dialog titled Record & summarize, with the three modes and Full screen chosen at a first start", async () => {
  await open();

  expect(screen.getByRole("dialog", { name: "Record & summarize" })).toBeInTheDocument();
  expect(["Full screen", "Single window", "Audio only"].map((name) => mode(name))).toHaveLength(3);
  expect(mode("Full screen")).toBeChecked();
  expect(mode("Single window")).not.toBeChecked();
  // The two sounds are on, the quality is the standard one.
  expect(screen.getByRole("checkbox", { name: "System audio (other people on a call, videos…)" })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: "Microphone (your voice)" })).toBeChecked();
  expect(screen.getByRole("radio", { name: "Standard" })).toBeChecked();
  expect(screen.getByText(/^15 frames\/s, up to 1920 px/)).toBeInTheDocument();
  expect(start()).toBeEnabled();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
});

test("start_is_disabled_until_a_window_is_chosen", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(mode("Single window"));
  const windows = screen.getByRole("combobox", { name: "Window" });

  // The first entry is no window: the person has to choose one.
  expect(within(windows).getAllByRole("option").map((option) => option.textContent)).toEqual([
    "Choose a window…",
    "Safari — Docs",
    "Zoom — Weekly sync",
  ]);
  expect(start()).toBeDisabled();
  await user.selectOptions(windows, "Zoom — Weekly sync");
  expect(start()).toBeEnabled();
  await user.selectOptions(windows, "Choose a window…");
  expect(start()).toBeDisabled();
});

test("audio_only_hides_the_quality_and_needs_one_source", async () => {
  const user = userEvent.setup();
  await open();
  expect(screen.getByRole("radiogroup", { name: "Video quality" })).toBeInTheDocument();

  await user.click(mode("Audio only"));

  expect(screen.queryByRole("radiogroup", { name: "Video quality" })).not.toBeInTheDocument();
  expect(screen.queryByText(/frames\/s/)).not.toBeInTheDocument();
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  expect(start()).toBeEnabled();
  await user.click(screen.getByRole("checkbox", { name: /System audio/ }));
  expect(start()).toBeEnabled();
  await user.click(screen.getByRole("checkbox", { name: /Microphone/ }));
  expect(start()).toBeDisabled();
  // A screen recording with both sounds off is a video with no sound: it is allowed.
  await user.click(mode("Full screen"));
  expect(start()).toBeEnabled();
});

test("a quality has its own line of words", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(screen.getByRole("radio", { name: "Small" }));
  expect(screen.getByText(/^10 frames\/s, up to 1280 px/)).toBeInTheDocument();
  await user.click(screen.getByRole("radio", { name: "Sharp" }));
  expect(screen.getByText(/^30 frames\/s, up to 3840 px/)).toBeInTheDocument();
});

test("needs_ffmpeg_disables_the_screen_modes_and_shows_the_command", async () => {
  const user = userEvent.setup();
  await open(fakeSources({ screen: "needs_ffmpeg", displays: [], windows: [] }));

  expect(screen.getByText("Recording the screen needs ffmpeg: brew install ffmpeg")).toBeInTheDocument();
  expect(mode("Full screen")).toBeDisabled();
  expect(mode("Single window")).toBeDisabled();
  // Sound alone needs no ffmpeg: it is what is left, and it is chosen.
  expect(mode("Audio only")).toBeEnabled();
  expect(mode("Audio only")).toBeChecked();
  expect(start()).toBeEnabled();

  await user.click(screen.getByRole("button", { name: "Copy install command" }));
  expect(api.copyText).toHaveBeenCalledExactlyOnceWith("brew install ffmpeg");
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
});

test("an ffmpeg with no video encoder is told the same way, with the command to install a full one", async () => {
  await open(fakeSources({ screen: "no_video_encoder", displays: [], windows: [] }));

  expect(
    screen.getByText("This ffmpeg has no video encoder that works on this computer. Install a full build: brew install ffmpeg"),
  ).toBeInTheDocument();
  expect(mode("Full screen")).toBeDisabled();
  expect(screen.getByRole("button", { name: "Copy install command" })).toBeInTheDocument();
});

test("a desktop that can't share the screen says so, and has no command to copy", async () => {
  await open(fakeSources({ screen: "portal_unavailable", displays: [], windows: [] }));

  expect(screen.getByText("This desktop can't share the screen with apps.")).toBeInTheDocument();
  expect(mode("Full screen")).toBeDisabled();
  expect(mode("Single window")).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Copy install command" })).not.toBeInTheDocument();
});

test("a Screen Recording that is off is told, with a button to System Settings, and the modes stay open", async () => {
  const user = userEvent.setup();
  await open(fakeSources({ screen: "needs_permission", displays: [], windows: [] }));

  expect(screen.getByText("Turn on Screen Recording for Dictate first.")).toBeInTheDocument();
  expect(mode("Full screen")).toBeEnabled();
  expect(start()).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "Open System Settings" }));
  expect(api.openPermissionSettings).toHaveBeenCalledExactlyOnceWith("screen");
});

test("the_last_choices_come_back", async () => {
  const user = userEvent.setup();
  await open(
    fakeSources({
      last: { mode: { kind: "audio" }, quality: "sharp", mic: false, system_audio: true },
    }),
  );

  expect(mode("Audio only")).toBeChecked();
  expect(screen.getByRole("checkbox", { name: /Microphone/ })).not.toBeChecked();
  expect(screen.getByRole("checkbox", { name: /System audio/ })).toBeChecked();
  // The quality is not shown for sound alone, and is not forgotten.
  expect(screen.queryByRole("radiogroup", { name: "Video quality" })).not.toBeInTheDocument();
  await user.click(mode("Full screen"));
  expect(screen.getByRole("radio", { name: "Sharp" })).toBeChecked();
});

test("the display of the last time comes back when it is still there", async () => {
  await open(
    fakeSources({
      displays: [
        { id: "1", name: "Display 1", app: null },
        { id: "2", name: "Display 2", app: null },
      ],
      last: { mode: { kind: "screen", display: "2" }, quality: "standard", mic: true, system_audio: true },
    }),
  );

  expect(screen.getByRole("combobox", { name: "Display" })).toHaveValue("2");
});

test("the last mode was a window: it comes back as the window mode, with no window chosen", async () => {
  await open(
    fakeSources({
      last: { mode: { kind: "window", id: "" }, quality: "small", mic: true, system_audio: true },
    }),
  );

  expect(mode("Single window")).toBeChecked();
  expect(screen.getByRole("combobox", { name: "Window" })).toHaveValue("");
  expect(start()).toBeDisabled();
});

test("more than one display has a select of them, and one display has none", async () => {
  const user = userEvent.setup();
  await open(
    fakeSources({
      displays: [
        { id: "1", name: "Display 1 (2560x1440)", app: null },
        { id: "2", name: "Display 2 (1920x1080)", app: null },
      ],
    }),
  );

  const displays = screen.getByRole("combobox", { name: "Display" });
  expect(within(displays).getAllByRole("option").map((option) => option.textContent)).toEqual([
    "Display 1 (2560x1440)",
    "Display 2 (1920x1080)",
  ]);
  expect(displays).toHaveValue("1");
  await user.selectOptions(displays, "Display 2 (1920x1080)");
  await user.click(start());
  expect(api.recordingStart).toHaveBeenCalledWith(expect.objectContaining({ mode: { kind: "screen", display: "2" } }));
});

test("one display has nothing to choose", async () => {
  await open();

  expect(screen.queryByRole("combobox", { name: "Display" })).not.toBeInTheDocument();
});

test("the_linux_picker_says_the_system_asks", async () => {
  const user = userEvent.setup();
  await open(fakeSources({ pickerIsTheSystem: true, displays: [], windows: [] }));

  // Both selects give way to the note: the desktop's own dialog is where the person chooses.
  expect(screen.getByText("Your system will ask what to share.")).toBeInTheDocument();
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  expect(screen.getByText("Dictate's own windows can appear in the video.")).toBeInTheDocument();
  await user.click(mode("Single window"));
  expect(screen.getByText("Your system will ask what to share.")).toBeInTheDocument();
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  // No window is chosen here: the desktop's dialog will.
  expect(start()).toBeEnabled();
  await user.click(start());
  expect(api.recordingStart).toHaveBeenCalledWith(expect.objectContaining({ mode: { kind: "window", id: "" } }));
});

test("a system that lists what there is has no note about the system asking, or about the windows of Dictate", async () => {
  await open();

  expect(screen.queryByText("Your system will ask what to share.")).not.toBeInTheDocument();
  expect(screen.queryByText("Dictate's own windows can appear in the video.")).not.toBeInTheDocument();
});

test("start_sends_the_chosen_options", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(mode("Single window"));
  await user.selectOptions(screen.getByRole("combobox", { name: "Window" }), "Zoom — Weekly sync");
  await user.click(screen.getByRole("radio", { name: "Sharp" }));
  await user.click(screen.getByRole("checkbox", { name: /System audio/ }));
  await user.click(start());

  expect(api.recordingStart).toHaveBeenCalledExactlyOnceWith({
    mode: { kind: "window", id: "77" },
    quality: "sharp",
    mic: true,
    system_audio: false,
  });
  // A recording that has begun is the bar's: the picker goes.
  await vi.waitFor(() => expect(store.recorder).toBeNull());
});

test("the screen is sent as the main display unless another one is chosen, and sound alone as audio", async () => {
  const user = userEvent.setup();
  const first = await open();

  await user.click(start());
  expect(api.recordingStart).toHaveBeenLastCalledWith({
    mode: { kind: "screen", display: null },
    quality: "standard",
    mic: true,
    system_audio: true,
  });

  first.unmount();
  await open();
  await user.click(mode("Audio only"));
  await user.click(screen.getByRole("checkbox", { name: /Microphone/ }));
  await user.click(start());
  expect(api.recordingStart).toHaveBeenLastCalledWith({
    mode: { kind: "audio" },
    quality: "standard",
    mic: false,
    system_audio: true,
  });
});

test("while it starts, it waits: nothing is sent twice and it can't be closed", async () => {
  const user = userEvent.setup();
  let begun: () => void = () => {};
  await open();
  vi.mocked(api.recordingStart).mockImplementation(() => new Promise<void>((resolve) => (begun = resolve)));

  await user.click(start());

  expect(screen.getByRole("button", { name: "Starting…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  await user.keyboard("{Escape}");
  await user.keyboard("{Enter}");
  expect(store.recorder).not.toBeNull();
  expect(api.recordingStart).toHaveBeenCalledTimes(1);

  begun();
  await vi.waitFor(() => expect(store.recorder).toBeNull());
});

test("an_error_is_shown_under_the_buttons", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.recordingStart).mockRejectedValue(
    fakeError({ key: "recording.error.source", detail: "no display found" }),
  );

  await user.click(start());

  expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't start recording: no display found");
  // The picker stays, to try again.
  expect(store.recorder).not.toBeNull();
  expect(start()).toBeEnabled();
  // The error is under the buttons, not above the choices.
  expect(start().compareDocumentPosition(screen.getByRole("alert")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("a start that fails for a missing tool has the command to copy next to the error", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.recordingStart).mockRejectedValue(
    fakeError({ kind: "MissingTool", key: "recording.error.needs_ffmpeg", detail: null, command: "brew install ffmpeg" }),
  );

  await user.click(start());

  expect(await screen.findByRole("alert")).toHaveTextContent("Recording the screen needs ffmpeg: brew install ffmpeg");
  await user.click(screen.getByRole("button", { name: "Copy install command" }));
  expect(api.copyText).toHaveBeenCalledExactlyOnceWith("brew install ffmpeg");
});

test("a start that fails for the want of parec (Linux, system audio on) has the command to install it next to the error", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.recordingStart).mockRejectedValue(
    fakeError({ kind: "MissingTool", key: "recording.error.needs_parec", detail: null, command: "sudo apt install pulseaudio-utils" }),
  );

  await user.click(start());

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Recording the system's audio needs parec. Turn system audio off, or install it: sudo apt install pulseaudio-utils",
  );
  await user.click(screen.getByRole("button", { name: "Copy install command" }));
  expect(api.copyText).toHaveBeenCalledExactlyOnceWith("sudo apt install pulseaudio-utils");
});

test("a start that fails for the permission has the button to System Settings next to the error", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.recordingStart).mockRejectedValue(
    fakeError({ key: "recording.error.needs_screen_permission", detail: null, command: "open_settings" }),
  );

  await user.click(start());

  expect(await screen.findByRole("alert")).toHaveTextContent("No Screen Recording permission.");
  await user.click(screen.getByRole("button", { name: "Open System Settings" }));
  expect(api.openPermissionSettings).toHaveBeenCalledExactlyOnceWith("screen");
});

test("the error that the picker was asked to show (a start from the shortcut that failed) is there when it opens", async () => {
  await open(fakeSources(), fakeError({ key: "recording.error.nothing_to_record", detail: null }));

  expect(screen.getByRole("alert")).toHaveTextContent("Turn on at least system audio or the microphone.");
});

test("an error that comes while the picker is open is shown, and a new try clears it", async () => {
  const user = userEvent.setup();
  await open();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();

  store.openRecorder(fakeError({ key: "recording.error.portal_cancelled", detail: null }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Nothing was chosen to share, so the recording did not start.");

  vi.mocked(api.recordingStart).mockImplementation(() => new Promise<void>(() => {}));
  await user.click(start());
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("Cancel and Esc close it without starting anything", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(store.recorder).toBeNull();

  store.openRecorder();
  await user.keyboard("{Escape}");
  expect(store.recorder).toBeNull();
  expect(api.recordingStart).not.toHaveBeenCalled();
});

test("Enter starts the recording, except where it presses a button or opens a select", async () => {
  const user = userEvent.setup();
  await open();

  mode("Full screen").focus();
  await user.keyboard("{Enter}");

  expect(api.recordingStart).toHaveBeenCalledTimes(1);
});

test("Enter on Cancel cancels: it is not a start", async () => {
  const user = userEvent.setup();
  await open();

  screen.getByRole("button", { name: "Cancel" }).focus();
  await user.keyboard("{Enter}");

  expect(api.recordingStart).not.toHaveBeenCalled();
  expect(store.recorder).toBeNull();
});

test("Enter does not start what can't start", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(mode("Single window"));

  mode("Single window").focus();
  await user.keyboard("{Enter}");

  expect(api.recordingStart).not.toHaveBeenCalled();
});

test("the caption names the shortcut as the system writes it", async () => {
  await open();
  expect(screen.getByText("Ctrl+Alt+R starts/stops anywhere, with these choices")).toBeInTheDocument();
});

test("on a Mac the caption writes the shortcut with the symbols of the keys", async () => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
  await open();

  expect(screen.getByText("⌃⌥R starts/stops anywhere, with these choices")).toBeInTheDocument();
});

test("a list of what there is to record that can't be read says why, and can still be closed", async () => {
  const user = userEvent.setup();
  stubRecordingApi();
  vi.mocked(api.recordingSources).mockRejectedValue("The system did not answer.");
  store = new MeetingsStore();
  store.openRecorder();
  render(RecordPicker, { store });

  expect(await screen.findByRole("alert")).toHaveTextContent("The system did not answer.");
  expect(screen.queryByRole("button", { name: "Start recording" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(store.recorder).toBeNull();
});

test("vietnamese_labels", async () => {
  setLanguage("vi");
  await open(
    fakeSources({
      displays: [
        { id: "1", name: "Display 1", app: null },
        { id: "2", name: "Display 2", app: null },
      ],
    }),
  );

  expect(screen.getByRole("dialog", { name: "Quay & tóm tắt" })).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: "Toàn màn hình" })).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: "Một cửa sổ" })).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: "Chỉ âm thanh" })).toBeInTheDocument();
  expect(screen.getByRole("combobox", { name: "Màn hình" })).toBeInTheDocument();
  expect(screen.getByRole("radiogroup", { name: "Chất lượng video" })).toBeInTheDocument();
  expect(["Nhẹ", "Chuẩn", "Nét"].map((name) => screen.getByRole("radio", { name }))).toHaveLength(3);
  expect(screen.getByRole("checkbox", { name: "Âm thanh hệ thống (tiếng người khác trong cuộc gọi, video…)" })).toBeInTheDocument();
  expect(screen.getByRole("checkbox", { name: "Micro (giọng của bạn)" })).toBeInTheDocument();
  expect(screen.getByText("Ctrl+Alt+R bắt đầu/dừng ở mọi nơi, dùng lựa chọn này")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Huỷ" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Bắt đầu quay" })).toBeInTheDocument();
});

test("vietnamese: the window select and the note that the system asks", async () => {
  const user = userEvent.setup();
  setLanguage("vi");
  await open();
  await user.click(screen.getByRole("radio", { name: "Một cửa sổ" }));

  expect(within(screen.getByRole("combobox", { name: "Cửa sổ" })).getAllByRole("option")[0]).toHaveTextContent("Chọn cửa sổ…");
});
