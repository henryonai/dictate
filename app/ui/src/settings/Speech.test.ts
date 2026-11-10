import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { ModelView, ToolsView } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { freezeTime, loadedStore, passTime, stubApi } from "../test-helpers";
import Speech from "./Speech.svelte";

const tiny: ModelView = { id: "tiny", size_mb: 75, downloaded: false, recommended: false };
const small: ModelView = { id: "small", size_mb: 466, downloaded: true, recommended: true };

/** The tools as the Rust side says them on a Mac: ffmpeg is installed, yt-dlp is not. */
function tools(overrides: Partial<ToolsView> = {}): ToolsView {
  return {
    ffmpeg: "7.1",
    ytDlp: null,
    ffmpegCommand: "brew install ffmpeg",
    ytDlpCommand: "brew install yt-dlp ffmpeg",
    ...overrides,
  };
}

let backend: ReturnType<typeof stubApi>;

beforeEach(() => {
  backend = stubApi();
  vi.mocked(api.listModels).mockResolvedValue([tiny, small]);
  vi.mocked(api.modelsDiskUsageMb).mockResolvedValue(466);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  setLanguage("en");
});

const lastSaved = () => vi.mocked(api.saveSettings).mock.lastCall?.[0];

async function row(name: string) {
  return within(await screen.findByRole("listitem", { name }));
}

function later<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test("lists the models with size, localized note and the recommended badge", async () => {
  render(Speech, { store: await loadedStore() });

  const tinyRow = await row("Tiny");
  expect(tinyRow.getByText("75 MB", { exact: false })).toBeInTheDocument();
  expect(tinyRow.getByText("Fastest, weak for Vietnamese", { exact: false })).toBeInTheDocument();
  expect(tinyRow.queryByText("Recommended")).not.toBeInTheDocument();

  const smallRow = await row("Small");
  expect(smallRow.getByText("466 MB", { exact: false })).toBeInTheDocument();
  expect(smallRow.getByText("Recommended")).toBeInTheDocument();
});

test("the notes follow the UI language", async () => {
  render(Speech, { store: await loadedStore({ ui_language: "vi" }) });
  const tinyRow = await row("Tiny");
  expect(tinyRow.getByText("Nhanh nhất, kém tiếng Việt", { exact: false })).toBeInTheDocument();
  expect(tinyRow.getByRole("button", { name: "Tải" })).toBeInTheDocument();
});

test("shows the space the models take", async () => {
  render(Speech, { store: await loadedStore() });
  expect(await screen.findByText("Downloaded models")).toBeInTheDocument();
  await vi.waitFor(() => expect(screen.getByTestId("disk-usage")).toHaveTextContent("466 MB"));
});

test("a model that isn't downloaded has Download, a downloaded one has Delete", async () => {
  render(Speech, { store: await loadedStore() });
  const tinyRow = await row("Tiny");
  expect(tinyRow.getByRole("button", { name: "Download" })).toBeEnabled();
  expect(tinyRow.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();

  const smallRow = await row("Small");
  expect(smallRow.getByRole("button", { name: "Delete" })).toBeEnabled();
  expect(smallRow.queryByRole("button", { name: "Download" })).not.toBeInTheDocument();
});

test("Download starts it and shows a progress bar with Cancel", async () => {
  const user = userEvent.setup();
  vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);
  render(Speech, { store: await loadedStore() });
  const tinyRow = await row("Tiny");

  await user.click(tinyRow.getByRole("button", { name: "Download" }));
  expect(api.downloadModel).toHaveBeenCalledWith("tiny");

  backend.emitDownload({ id: "tiny", done: 15, total: 75, state: "running" });
  const bar = await tinyRow.findByRole("progressbar");
  expect(bar).toHaveAttribute("value", "15");
  expect(bar).toHaveAttribute("max", "75");
  expect(tinyRow.getByRole("button", { name: "Cancel" })).toBeEnabled();
  expect(tinyRow.queryByRole("button", { name: "Download" })).not.toBeInTheDocument();
});

test("Cancel shows Cancelling… (disabled) until the cancelled event arrives", async () => {
  freezeTime();
  const user = userEvent.setup();
  vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);
  render(Speech, { store: await loadedStore() });
  const tinyRow = await row("Tiny");
  await user.click(tinyRow.getByRole("button", { name: "Download" }));
  passTime(1000);

  await user.click(await tinyRow.findByRole("button", { name: "Cancel" }));

  expect(api.cancelDownload).toHaveBeenCalledWith("tiny");
  expect(await tinyRow.findByRole("button", { name: "Cancelling…" })).toBeDisabled();
  expect(tinyRow.queryByRole("progressbar")).not.toBeInTheDocument();

  backend.emitDownload({ id: "tiny", done: 15, total: 75, state: "cancelled" });
  expect(await tinyRow.findByRole("button", { name: "Download" })).toBeEnabled();
});

test("the second click of a double click on Download doesn't land on Cancel", async () => {
  freezeTime();
  const user = userEvent.setup();
  vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);
  render(Speech, { store: await loadedStore() });
  const tinyRow = await row("Tiny");

  await user.click(tinyRow.getByRole("button", { name: "Download" }));
  // Cancel is where Download was, so the second click of the double click hits it.
  await user.click(await tinyRow.findByRole("button", { name: "Cancel" }));

  expect(api.cancelDownload).not.toHaveBeenCalled();
  expect(tinyRow.getByRole("progressbar")).toBeInTheDocument();
  expect(api.downloadModel).toHaveBeenCalledTimes(1);
});

test("a failed download shows the error and Retry", async () => {
  const user = userEvent.setup();
  const download = later<void>();
  vi.mocked(api.downloadModel).mockReturnValueOnce(download.promise);
  render(Speech, { store: await loadedStore() });
  const tinyRow = await row("Tiny");
  await user.click(tinyRow.getByRole("button", { name: "Download" }));

  backend.emitDownload({ id: "tiny", done: 1, total: 75, state: "failed", error: "API error 503: busy" });
  download.reject("API error 503: busy");

  expect(await tinyRow.findByText("API error 503: busy", { exact: false })).toBeInTheDocument();
  vi.mocked(api.downloadModel).mockReturnValueOnce(later<void>().promise);
  await user.click(tinyRow.getByRole("button", { name: "Retry" }));
  expect(api.downloadModel).toHaveBeenCalledTimes(2);
  expect(await tinyRow.findByRole("button", { name: "Cancel" })).toBeInTheDocument();
});

test("Delete removes the model", async () => {
  const user = userEvent.setup();
  render(Speech, { store: await loadedStore() });
  const smallRow = await row("Small");

  await user.click(smallRow.getByRole("button", { name: "Delete" }));

  expect(api.deleteModel).toHaveBeenCalledWith("small");
});

test("Delete is disabled while a download of that model runs", async () => {
  render(Speech, { store: await loadedStore() });
  const smallRow = await row("Small");
  expect(smallRow.getByRole("button", { name: "Delete" })).toBeEnabled();

  backend.emitDownload({ id: "small", done: 1, total: 466, state: "running" });

  await vi.waitFor(() => expect(smallRow.getByRole("button", { name: "Delete" })).toBeDisabled());
});

test("shows why the models can't be listed", async () => {
  vi.mocked(api.listModels).mockRejectedValue("No data folder");
  render(Speech, { store: await loadedStore() });
  expect(await screen.findByText("No data folder")).toBeInTheDocument();
});

test("the radio marks the model in use: the recommended one until another is chosen", async () => {
  const user = userEvent.setup();
  render(Speech, { store: await loadedStore() });

  expect(await screen.findByRole("radio", { name: "Use Small" })).toBeChecked();
  await user.click(screen.getByRole("radio", { name: "Use Tiny" }));

  expect(lastSaved()).toMatchObject({ whisper_model: "tiny" });
  expect(screen.getByRole("radio", { name: "Use Tiny" })).toBeChecked();
});

test("warns when the model in use isn't downloaded", async () => {
  render(Speech, { store: await loadedStore({ whisper_model: "tiny" }) });
  expect(await screen.findByText("The selected model isn't downloaded yet.")).toBeInTheDocument();
});

test("no warning when the model in use is there", async () => {
  render(Speech, { store: await loadedStore({ whisper_model: "small" }) });
  await screen.findByRole("listitem", { name: "Small" });
  expect(screen.queryByText("The selected model isn't downloaded yet.")).not.toBeInTheDocument();
});

test("the engines for dictation and for files are picked apart", async () => {
  const user = userEvent.setup();
  render(Speech, { store: await loadedStore() });

  await user.selectOptions(screen.getByLabelText("Dictation engine"), "Transcription API");
  expect(lastSaved()).toMatchObject({ dictation_engine: "api", file_engine: "whisper" });

  await user.selectOptions(screen.getByLabelText("File engine"), "Transcription API");
  expect(lastSaved()).toMatchObject({ dictation_engine: "api", file_engine: "api" });
});

test("the transcription language is auto-detect, a known language or an ISO code", async () => {
  const user = userEvent.setup();
  render(Speech, { store: await loadedStore() });

  const language = screen.getByLabelText("Transcription language");
  expect(language).toHaveDisplayValue("Auto-detect");
  await user.selectOptions(language, "Tiếng Việt");
  expect(lastSaved()).toMatchObject({ transcribe_language: "vi" });

  await user.selectOptions(language, "Other…");
  await user.type(screen.getByLabelText("Language code (ISO 639-1)"), "pt");
  await user.tab();
  expect(lastSaved()).toMatchObject({ transcribe_language: "pt" });
});

test("the speaker separation switch saves", async () => {
  const user = userEvent.setup();
  render(Speech, { store: await loadedStore() });

  await user.click(screen.getByRole("checkbox", { name: "Separate speakers (Speaker 1, 2…)" }));

  expect(lastSaved()).toMatchObject({ diarize: false });
});

test("has no test with a WAV file any more: the meetings window transcribes files", async () => {
  render(Speech, { store: await loadedStore() });

  expect(screen.queryByLabelText("Path to a WAV file")).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "Test with a WAV file" })).not.toBeInTheDocument();
});

test("diarize_hint_says_46_mb: the switch says what the first use downloads", async () => {
  render(Speech, { store: await loadedStore() });

  expect(screen.getByText("The first time, a speaker separation model (about 46 MB) is downloaded too.")).toBeInTheDocument();
});

test("the hint of the speaker separation is Vietnamese in Vietnamese", async () => {
  render(Speech, { store: await loadedStore({ ui_language: "vi" }) });

  expect(screen.getByText("Lần đầu dùng sẽ tải thêm model tách người nói, khoảng 46 MB.")).toBeInTheDocument();
});

test("speech_settings_show_tool_status_and_install_command: what is there with its version, and what is missing with the command", async () => {
  vi.mocked(api.mediaTools).mockResolvedValue(tools());
  render(Speech, { store: await loadedStore() });

  const group = await screen.findByRole("group", { name: "Tools for links" });
  expect(await within(group).findByText("ffmpeg ✓ 7.1")).toBeInTheDocument();
  expect(within(group).getByText("yt-dlp ✗ missing")).toBeInTheDocument();
  expect(within(group).getByText("brew install yt-dlp ffmpeg")).toBeInTheDocument();
  // ffmpeg is there: nothing to install for it.
  expect(within(group).queryByText("brew install ffmpeg")).not.toBeInTheDocument();
  expect(api.mediaTools).toHaveBeenCalled();
});

test("each tool that is missing has its own command, for the system the app runs on", async () => {
  vi.mocked(api.mediaTools).mockResolvedValue(
    tools({ ffmpeg: null, ffmpegCommand: "sudo apt install ffmpeg", ytDlpCommand: "pipx install yt-dlp && sudo apt install ffmpeg" }),
  );
  render(Speech, { store: await loadedStore() });

  const group = await screen.findByRole("group", { name: "Tools for links" });
  expect(await within(group).findByText("ffmpeg ✗ missing")).toBeInTheDocument();
  expect(within(group).getByText("yt-dlp ✗ missing")).toBeInTheDocument();
  expect(within(group).getByText("sudo apt install ffmpeg")).toBeInTheDocument();
  expect(within(group).getByText("pipx install yt-dlp && sudo apt install ffmpeg")).toBeInTheDocument();
});

test("two tools that are there show their versions and no command", async () => {
  vi.mocked(api.mediaTools).mockResolvedValue(tools({ ytDlp: "2025.01.15" }));
  render(Speech, { store: await loadedStore() });

  const group = await screen.findByRole("group", { name: "Tools for links" });
  expect(await within(group).findByText("yt-dlp ✓ 2025.01.15")).toBeInTheDocument();
  expect(within(group).getByText("ffmpeg ✓ 7.1")).toBeInTheDocument();
  expect(within(group).queryByText(/Run:/)).not.toBeInTheDocument();
  expect(group.querySelector("code")).toBeNull();
});

test("a tool that is there and does not say its version is there, with no version", async () => {
  vi.mocked(api.mediaTools).mockResolvedValue(tools({ ffmpeg: "unknown", ytDlp: "unknown" }));
  render(Speech, { store: await loadedStore() });

  const group = await screen.findByRole("group", { name: "Tools for links" });
  expect(await within(group).findByText("ffmpeg ✓")).toBeInTheDocument();
  expect(within(group).getByText("yt-dlp ✓")).toBeInTheDocument();
});

test("the tools are asked again when the window gets the focus back, as a tool is installed in another window", async () => {
  vi.mocked(api.mediaTools).mockResolvedValue(tools());
  render(Speech, { store: await loadedStore() });
  const group = await screen.findByRole("group", { name: "Tools for links" });
  expect(await within(group).findByText("yt-dlp ✗ missing")).toBeInTheDocument();

  vi.mocked(api.mediaTools).mockResolvedValue(tools({ ytDlp: "2025.01.15" }));
  await fireEvent.focus(window);

  expect(await within(group).findByText("yt-dlp ✓ 2025.01.15")).toBeInTheDocument();
  expect(group.querySelector("code")).toBeNull();
});

test("tools that can't be looked up say why, and the rest of the page is still there", async () => {
  vi.mocked(api.mediaTools).mockRejectedValue("Could not look for the tools.");
  render(Speech, { store: await loadedStore() });

  expect(await screen.findByText("Could not look for the tools.")).toBeInTheDocument();
  expect(screen.queryByText(/✗ missing/)).not.toBeInTheDocument();
  expect(await screen.findByRole("listitem", { name: "Small" })).toBeInTheDocument();
});

test("the tools are in Vietnamese in Vietnamese", async () => {
  vi.mocked(api.mediaTools).mockResolvedValue(tools());
  render(Speech, { store: await loadedStore({ ui_language: "vi" }) });

  const group = await screen.findByRole("group", { name: "Công cụ cho link" });
  expect(await within(group).findByText("ffmpeg ✓ 7.1")).toBeInTheDocument();
  expect(within(group).getByText("yt-dlp ✗ thiếu")).toBeInTheDocument();
  expect(within(group).getByText("Chạy:")).toBeInTheDocument();
});
