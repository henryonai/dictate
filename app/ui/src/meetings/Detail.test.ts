import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { MeetingDetail, MeetingRow } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeDetail, fakeError, fakeJob, fakeMedia, fakeRow, fakeSummary, stubMeetingsApi } from "../test-helpers";
import Detail from "./Detail.svelte";
import LogSheet from "./LogSheet.svelte";

const ID = "2026-03-02 10-00-00";
const OTHER = "2026-03-01 09-00-00";
const rows = [fakeRow({ id: ID, title: "Weekly planning" }), fakeRow({ id: OTHER, title: "Retro" })];
const MORE = "More: summarize again, open folder, view log, delete";
const FIRST_URL = "http://127.0.0.1:41000/token/1";
const NEW_URL = "http://127.0.0.1:52000/other-token/1";

let events: ReturnType<typeof stubMeetingsApi>;
let store: MeetingsStore;
const scrollIntoView = vi.fn();

/** A detail with what a finished recording has: a summary, speakers and a video. */
function whole(overrides: Partial<MeetingDetail> = {}): MeetingDetail {
  return fakeDetail({
    row: rows[0],
    summary: fakeSummary(),
    media: fakeMedia(),
    speakers: [{ name: "Speaker 1", lines: 1, sample: { start: 0, stamp: "00:00", speaker: "Speaker 1", text: "Let's start." } }],
    ...overrides,
  });
}

/** The detail of the first recording, selected in a store, and shown with the sheet of the log that the window has beside it. */
async function opened(detail: MeetingDetail = whole(), library: MeetingRow[] = rows) {
  events = stubMeetingsApi(library);
  vi.mocked(api.meetingDetail).mockImplementation(async (id) =>
    id === detail.row.id ? detail : fakeDetail({ row: library.find((row) => row.id === id) ?? fakeRow({ id }) }),
  );
  store = new MeetingsStore();
  await store.start();
  await store.select(ID);
  const view = render(Detail, { store });
  render(LogSheet, { store });
  await screen.findByRole("heading", { level: 1, name: "Weekly planning" });
  return { user: userEvent.setup(), view };
}

const button = (name: string) => screen.getByRole("button", { name });
const video = () => document.querySelector("video") as HTMLVideoElement;
const part = (name: string) => screen.getByRole("heading", { name }).closest("section") as HTMLElement;
const menuItems = (name: string) => within(screen.getByRole("group", { name })).getAllByRole("button").map((item) => item.textContent?.trim());

beforeEach(() => {
  // jsdom has no media: the calls are what the window asks of the element.
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
  Element.prototype.scrollTo = vi.fn() as unknown as typeof Element.prototype.scrollTo;
  Element.prototype.scrollIntoView = scrollIntoView;
});

afterEach(() => {
  store?.stop();
  scrollIntoView.mockReset();
  vi.restoreAllMocks();
  setLanguage("en");
});

// ---- The body

test("a finished recording is a split view: the player and the transcript on the left, the summary on the right", async () => {
  await opened();

  const left = part("Transcript").closest(".left");
  expect(left).not.toBeNull();
  expect(video().closest(".left")).toBe(left);
  const right = part("Key points").closest(".right");
  expect(right).not.toBeNull();
  expect(right).not.toBe(left);
  expect(button("Copy summary")).toBeInTheDocument();
});

test("the player is a video for a recording with a picture, and an audio element for one without", async () => {
  const first = await opened(whole({ media: fakeMedia({ video: true }) }));
  expect(video().getAttribute("src")).toBe(FIRST_URL);
  expect(document.querySelector("audio")).toBeNull();
  first.view.unmount();

  await opened(whole({ media: fakeMedia({ video: false, url: "http://127.0.0.1:41000/token/2" }) }));
  expect(document.querySelector("audio")).not.toBeNull();
  expect(document.querySelector("video")).toBeNull();
});

test("a recording with no media has no player, and the rest is shown", async () => {
  await opened(whole({ media: null }));

  expect(document.querySelector("video, audio")).toBeNull();
  expect(screen.getByRole("heading", { name: "Transcript" })).toBeInTheDocument();
  expect(screen.queryByText("follows the video")).not.toBeInTheDocument();
  // Nothing to seek: the stamps and the chapters are off.
  expect(button("01:05")).toBeDisabled();
  expect(button("01:05 Budget")).toBeDisabled();
});

test("missing_media_message: a file that was moved or deleted says so, with a player (audio.m4a) or without", async () => {
  const first = await opened(whole({ mediaMissing: true, media: fakeMedia({ video: false }) }));
  expect(screen.getByText("The original file was moved or deleted.")).toBeInTheDocument();
  expect(document.querySelector("audio")).not.toBeNull();
  first.view.unmount();

  await opened(whole({ mediaMissing: true, media: null }));
  expect(screen.getByText("The original file was moved or deleted.")).toBeInTheDocument();
  expect(document.querySelector("video, audio")).toBeNull();
});

test("a recording whose file is there says nothing of it", async () => {
  await opened();
  expect(screen.queryByText("The original file was moved or deleted.")).not.toBeInTheDocument();
});

test("Linux without a decoder for AAC says which package to install, and the other systems say nothing", async () => {
  const text = "Install gstreamer1.0-libav to play recordings.";
  const platform = vi.spyOn(navigator, "platform", "get");
  const canPlay = vi.spyOn(HTMLAudioElement.prototype, "canPlayType");

  platform.mockReturnValue("Linux x86_64");
  canPlay.mockReturnValue("");
  const linux = await opened();
  expect(screen.getByText(text)).toBeInTheDocument();
  expect(canPlay).toHaveBeenCalledWith('audio/mp4; codecs="mp4a.40.2"');
  linux.view.unmount();

  canPlay.mockReturnValue("maybe");
  const decoded = await opened();
  expect(screen.queryByText(text)).not.toBeInTheDocument();
  decoded.view.unmount();

  platform.mockReturnValue("MacIntel");
  canPlay.mockReturnValue("");
  await opened();
  expect(screen.queryByText(text)).not.toBeInTheDocument();
});

test("a file that the element can't play (an error with the address that is right) is said, and the element is put away", async () => {
  await opened(whole({ media: fakeMedia({ video: false }) }));

  document.querySelector("audio")?.dispatchEvent(new Event("error"));

  expect(await screen.findByText("This recording can't be played here. Open it in another app.")).toBeInTheDocument();
  expect(document.querySelector("audio")).toBeNull();
  // The chapters have no player any more.
  expect(button("01:05")).toBeDisabled();
});

test("an error of the element reads the detail again and plays from the address that it has now", async () => {
  await opened();
  const element = video();
  const reads = vi.mocked(api.meetingDetail).mock.calls.length;
  vi.mocked(api.meetingDetail).mockResolvedValue(whole({ media: fakeMedia({ url: NEW_URL }) }));

  element.dispatchEvent(new Event("error"));

  await vi.waitFor(() => expect(element.getAttribute("src")).toBe(NEW_URL));
  expect(vi.mocked(api.meetingDetail).mock.calls.length).toBeGreaterThan(reads);
  // The same element goes on: it is not made again.
  expect(video()).toBe(element);
});

test("the detail that is read again by the window brings a new address to the player too", async () => {
  await opened();
  vi.mocked(api.meetingDetail).mockResolvedValue(whole({ media: fakeMedia({ url: NEW_URL }) }));

  events.emitChanged();

  await vi.waitFor(() => expect(video().getAttribute("src")).toBe(NEW_URL));
});

// ---- Seeking

test("stamp_click_seeks_player: a stamp of the transcript sets the position of the video and plays it", async () => {
  const { user } = await opened();

  await user.click(button("01:05"));

  await vi.waitFor(() => expect(HTMLMediaElement.prototype.play).toHaveBeenCalled());
  expect(video().currentTime).toBe(65);
});

test("chapters and the stamps of the texts seek the same player", async () => {
  const { user } = await opened();

  await user.click(button("01:05 Budget"));
  await vi.waitFor(() => expect(video().currentTime).toBe(65));

  await user.click(button("00:00 Kick-off"));
  await vi.waitFor(() => expect(video().currentTime).toBe(0));

  await user.click(within(part("Key points")).getByRole("link", { name: "[01:05]" }));
  await vi.waitFor(() => expect(video().currentTime).toBe(65));
});

test("a recording that was paused is looked at again before a seek: a new address is played from, at the second asked", async () => {
  const { user } = await opened();
  vi.mocked(api.meetingDetail).mockResolvedValue(whole({ media: fakeMedia({ url: NEW_URL }) }));

  await user.click(button("01:05"));

  await vi.waitFor(() => expect(video().getAttribute("src")).toBe(NEW_URL));
  video().dispatchEvent(new Event("loadedmetadata"));
  expect(video().currentTime).toBe(65);
  expect(HTMLMediaElement.prototype.play).toHaveBeenCalled();
});

// ---- The header and its menus

test("Copy summary copies the Markdown that the model wrote", async () => {
  const { user } = await opened(whole({ summary: fakeSummary({ markdown: "# T\n\n## Summary\nBody." }) }));

  await user.click(button("Copy summary"));

  expect(api.copyText).toHaveBeenCalledWith("# T\n\n## Summary\nBody.");
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
});

test("a recording with no summary has no Copy summary", async () => {
  await opened(whole({ summary: null }));
  expect(screen.queryByRole("button", { name: "Copy summary" })).not.toBeInTheDocument();
});

test("export_menu_calls_commands: save the file, copy the whole Markdown, copy the transcript", async () => {
  const { user } = await opened();

  await user.click(button("Export"));
  await user.click(button("Save Markdown file (summary + transcript)…"));
  expect(api.meetingExport).toHaveBeenCalledExactlyOnceWith(ID);
  // A choice closes the menu and the focus goes back to its button.
  expect(screen.queryByRole("button", { name: "Copy full Markdown" })).not.toBeInTheDocument();
  expect(button("Export")).toHaveFocus();

  vi.mocked(api.meetingMarkdown).mockResolvedValue("# Weekly planning\n\n---\n\n## Transcript");
  await user.click(button("Export"));
  await user.click(button("Copy full Markdown"));
  await vi.waitFor(() => expect(api.copyText).toHaveBeenCalledWith("# Weekly planning\n\n---\n\n## Transcript"));
  expect(api.meetingMarkdown).toHaveBeenCalledWith(ID);

  await user.click(await screen.findByRole("button", { name: "Copied" }));
  await user.click(button("Copy transcript"));
  expect(api.copyText).toHaveBeenLastCalledWith("[00:00] Speaker 1: Let's start.\n\n[01:05] Speaker 2: First, the budget.");
});

test("a failed export says why in the notice", async () => {
  const { user } = await opened();
  vi.mocked(api.meetingExport).mockRejectedValue(fakeError({ detail: "Could not write the file: disk full" }));

  await user.click(button("Export"));
  await user.click(button("Save Markdown file (summary + transcript)…"));

  await vi.waitFor(() => expect(store.notice).toBe("Could not write the file: disk full"));
});

test("with nothing to export the Export menu is off; with a summary only, the transcript can't be copied", async () => {
  const empty = await opened(whole({ summary: null, transcript: [] }));
  expect(button("Export")).toBeDisabled();
  empty.view.unmount();

  const { user } = await opened(whole({ transcript: [] }));
  await user.click(button("Export"));
  expect(button("Copy transcript")).toBeDisabled();
  expect(button("Save Markdown file (summary + transcript)…")).toBeEnabled();
});

test("the ⋯ menu of a recording that is not busy offers all of its choices", async () => {
  const { user } = await opened();

  await user.click(button(MORE));

  expect(menuItems(MORE)).toEqual([
    "Summarize again",
    "Transcribe + summarize again from scratch",
    "Rename speakers…",
    "Open folder",
    "Open video/audio in another app",
    "View log",
    "Delete recording (move to Trash)",
  ]);
});

test("menu_hides_rerun_while_busy: while a job runs there is no summarizing again nor renaming; the folder, the log and the delete stay", async () => {
  const { user } = await opened();
  events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.2 }));
  await screen.findByRole("list", { name: "Progress" });

  await user.click(button(MORE));

  expect(menuItems(MORE)).toEqual(["Open folder", "Open video/audio in another app", "View log", "Delete recording (move to Trash)"]);
});

test("the choices that need something are off without it: nothing to summarize without a transcript, no one to rename without speakers", async () => {
  const { user } = await opened(whole({ transcript: [], speakers: [] }));

  await user.click(button(MORE));

  expect(button("Summarize again")).toBeDisabled();
  expect(button("Rename speakers…")).toBeDisabled();
  expect(button("Transcribe + summarize again from scratch")).toBeEnabled();
});

test("a recording with no media has no choice to open it in another app", async () => {
  const { user } = await opened(whole({ media: null }));
  await user.click(button(MORE));
  expect(screen.queryByRole("button", { name: "Open video/audio in another app" })).not.toBeInTheDocument();
});

test("a recording whose file is gone has no choice to open it, even when its audio.m4a plays", async () => {
  const { user } = await opened(whole({ mediaMissing: true, media: fakeMedia({ video: false }) }));
  expect(document.querySelector("audio")).not.toBeNull();

  await user.click(button(MORE));

  expect(screen.queryByRole("button", { name: "Open video/audio in another app" })).not.toBeInTheDocument();
  expect(button("Open folder")).toBeInTheDocument();
});

test("a recording whose file is gone can't be transcribed again from scratch: there is nothing to start from", async () => {
  const gone = await opened(whole({ mediaMissing: true, media: null }));
  await gone.user.click(button(MORE));
  expect(button("Transcribe + summarize again from scratch")).toBeDisabled();
  // The summary can still be made again from the transcript that is kept.
  expect(button("Summarize again")).toBeEnabled();
  gone.view.unmount();

  const there = await opened();
  await there.user.click(button(MORE));
  expect(button("Transcribe + summarize again from scratch")).toBeEnabled();
});

test("the choices run their commands: summarize again, from scratch, the folder, the media", async () => {
  const { user } = await opened();

  await user.click(button(MORE));
  await user.click(button("Summarize again"));
  expect(api.meetingResummarize).toHaveBeenCalledExactlyOnceWith(ID);

  await user.click(button(MORE));
  await user.click(button("Transcribe + summarize again from scratch"));
  expect(api.meetingRerun).toHaveBeenCalledExactlyOnceWith(ID);

  await user.click(button(MORE));
  await user.click(button("Open folder"));
  expect(api.meetingOpenFolder).toHaveBeenCalledExactlyOnceWith(ID);

  await user.click(button(MORE));
  await user.click(button("Open video/audio in another app"));
  expect(api.meetingOpenMedia).toHaveBeenCalledExactlyOnceWith(ID);
});

test("a choice that is refused (the recording is busy) is a short notice", async () => {
  const { user } = await opened();
  vi.mocked(api.meetingResummarize).mockRejectedValue(fakeError({ key: "meetings.error.busy", detail: null }));

  await user.click(button(MORE));
  await user.click(button("Summarize again"));

  await vi.waitFor(() => expect(store.notice).toBe("This recording is being processed. Try again when it is done."));
});

test("Esc closes the menu and gives the focus back to its button; so does a press outside", async () => {
  const { user } = await opened();

  await user.click(button(MORE));
  expect(screen.getByRole("group", { name: MORE })).toBeInTheDocument();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("group", { name: MORE })).not.toBeInTheDocument();
  expect(button(MORE)).toHaveFocus();

  await user.click(button(MORE));
  await user.click(screen.getByRole("heading", { level: 1 }));
  expect(screen.queryByRole("group", { name: MORE })).not.toBeInTheDocument();
});

test("opening one menu leaves the other closed", async () => {
  const { user } = await opened();

  await user.click(button("Export"));
  await user.click(button(MORE));

  expect(screen.queryByRole("group", { name: "Export" })).not.toBeInTheDocument();
  expect(screen.getByRole("group", { name: MORE })).toBeInTheDocument();
});

test("View log opens the log, and closing it gives the focus back to the button of the menu", async () => {
  const { user } = await opened();

  await user.click(button(MORE));
  await user.click(button("View log"));
  const sheet = await screen.findByRole("dialog");
  expect(api.meetingLog).toHaveBeenCalledWith(ID);

  await user.click(within(sheet).getByRole("button", { name: "Close" }));
  await vi.waitFor(() => expect(button(MORE)).toHaveFocus());
});

// ---- Speakers

test("the name of a speaker, the button of the transcript and the choice of the menu ask to rename, and the sheet answers", async () => {
  const { user } = await opened();
  const line = screen.getByText("First, the budget.").closest("[data-line]") as HTMLElement;

  await user.click(within(line).getByRole("button", { name: "Speaker 2" }));
  expect(store.renameFor).toEqual({ speaker: "Speaker 2" });

  store.closeRename();
  await user.click(button("Rename speakers"));
  expect(store.renameFor).toEqual({ speaker: null });

  store.closeRename();
  await user.click(button(MORE));
  await user.click(button("Rename speakers…"));
  expect(store.renameFor).toEqual({ speaker: null });
});

/** The two speakers of the transcript of `fakeDetail`, as the Rust side lists them. */
const TWO_SPEAKERS = [
  { name: "Speaker 1", lines: 1, sample: { start: 0, stamp: "00:00", speaker: "Speaker 1", text: "Let's start." } },
  { name: "Speaker 2", lines: 1, sample: { start: 65, stamp: "01:05", speaker: "Speaker 2", text: "First, the budget." } },
];

test("the sheet opens on the speaker whose name was clicked, and Esc closes it and gives the focus back to that name", async () => {
  const { user } = await opened(whole({ speakers: TWO_SPEAKERS }));
  const line = screen.getByText("First, the budget.").closest("[data-line]") as HTMLElement;
  const name = within(line).getByRole("button", { name: "Speaker 2" });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

  await user.click(name);

  const sheet = await screen.findByRole("dialog", { name: "Rename speakers" });
  expect(within(sheet).getByRole("textbox", { name: "New name for Speaker 2" })).toHaveFocus();
  expect(within(sheet).getAllByRole("listitem")).toHaveLength(2);

  await user.keyboard("{Escape}");

  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(store.renameFor).toBeNull();
  expect(name).toHaveFocus();
});

test("giving the names changes the recording: the sheet closes, and the transcript read again has them", async () => {
  const { user } = await opened(whole({ speakers: TWO_SPEAKERS }));
  await user.click(button("Rename speakers"));
  const sheet = await screen.findByRole("dialog", { name: "Rename speakers" });
  expect(within(sheet).getByRole("textbox", { name: "New name for Speaker 1" })).toHaveFocus();

  await user.type(within(sheet).getByRole("textbox", { name: "New name for Speaker 1" }), "An");
  await user.click(within(sheet).getByRole("button", { name: "Rename" }));

  await vi.waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(api.meetingRenameSpeakers).toHaveBeenCalledExactlyOnceWith(ID, { "Speaker 1": "An" }, false);
  // The Rust side says that the library changed: the detail is read again, with what it wrote.
  vi.mocked(api.meetingDetail).mockResolvedValue(
    whole({
      speakers: [{ ...TWO_SPEAKERS[0], name: "An" }, TWO_SPEAKERS[1]],
      transcript: [
        { start: 0, stamp: "00:00", speaker: "An", text: "Let's start." },
        { start: 65, stamp: "01:05", speaker: "Speaker 2", text: "First, the budget." },
      ],
    }),
  );
  events.emitChanged();
  expect(await screen.findByRole("button", { name: "An" })).toBeInTheDocument();
});

test("Listen in the sheet seeks the player of the window, and closing the sheet stops the sample", async () => {
  const { user } = await opened(whole({ speakers: TWO_SPEAKERS }));
  await user.click(button("Rename speakers"));
  const sheet = await screen.findByRole("dialog", { name: "Rename speakers" });

  await user.click(within(sheet).getByRole("button", { name: "Listen to Speaker 2 from 01:05" }));

  await vi.waitFor(() => expect(video().currentTime).toBe(65));
  expect(HTMLMediaElement.prototype.play).toHaveBeenCalled();
  vi.mocked(HTMLMediaElement.prototype.pause).mockClear();
  await user.click(within(sheet).getByRole("button", { name: "Cancel" }));
  expect(HTMLMediaElement.prototype.pause).toHaveBeenCalledTimes(1);
});

test("a sheet is let go of when its recording starts to work, so that no dialog is left open with nothing in it", async () => {
  await opened(whole({ speakers: TWO_SPEAKERS }));
  store.openRename();
  await screen.findByRole("dialog", { name: "Rename speakers" });

  events.emitJob(fakeJob({ id: ID, step: "summary" }));

  await vi.waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(store.renameFor).toBeNull();
  expect(store.modalOpen).toBe(false);
});

test("a transcript of live captions has no speakers to rename", async () => {
  await opened(whole({ transcriptIsLive: true, speakers: [] }));

  expect(screen.queryByRole("button", { name: "Rename speakers" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Speaker 2" })).not.toBeInTheDocument();
  expect(screen.getByText("(draft)")).toBeInTheDocument();
});

// ---- Delete

test("delete releases the player first, then moves the recording to the Trash, with no question, and selects the first row", async () => {
  const { user } = await opened();
  const element = video();
  const seen: { src: string | null; paused: number; loaded: number }[] = [];
  vi.mocked(api.meetingDelete).mockImplementation(async () => {
    seen.push({
      src: element.getAttribute("src"),
      paused: vi.mocked(HTMLMediaElement.prototype.pause).mock.calls.length,
      loaded: vi.mocked(HTMLMediaElement.prototype.load).mock.calls.length,
    });
  });
  // After the delete the library has the other recording only.
  vi.mocked(api.meetingsList).mockResolvedValue([rows[1]]);

  await user.click(button(MORE));
  await user.click(button("Delete recording (move to Trash)"));

  await vi.waitFor(() => expect(api.meetingDelete).toHaveBeenCalledExactlyOnceWith(ID));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  // At the time of the delete the element had let go of its file.
  expect(seen).toEqual([{ src: null, paused: 1, loaded: 1 }]);
  await vi.waitFor(() => expect(store.selectedId).toBe(OTHER));
  await screen.findByRole("heading", { level: 1, name: "Retro" });
});

test("a delete that fails shows its detail, keeps the recording, and the player comes back", async () => {
  const { user } = await opened();
  vi.mocked(api.meetingDelete).mockRejectedValue(fakeError({ detail: "Could not move the recording to the Trash: in use" }));

  await user.click(button(MORE));
  await user.click(button("Delete recording (move to Trash)"));

  await vi.waitFor(() => expect(store.notice).toBe("Could not move the recording to the Trash: in use"));
  expect(screen.getByRole("heading", { level: 1, name: "Weekly planning" })).toBeInTheDocument();
  await vi.waitFor(() => expect(video()?.getAttribute("src")).toBe(FIRST_URL));
});

// ---- What else is in the detail

test("a failed job's card is above the split view, which keeps the transcript that was written", async () => {
  const error = fakeError({ kind: "Api", key: "meetings.error.summary_failed", detail: "API error 401" });
  await opened(whole({ row: { ...rows[0], error }, summary: null }), [{ ...rows[0], error }, rows[1]]);

  expect(screen.getByRole("heading", { name: "Couldn't summarize: API error" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Transcript" })).toBeInTheDocument();
  expect(screen.getByText("First, the budget.")).toBeInTheDocument();
  expect(screen.getByText("No summary yet")).toBeInTheDocument();
});

test("while a job runs the progress is shown and the split view is not", async () => {
  await opened();
  events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.2 }));

  await screen.findByRole("list", { name: "Progress" });
  expect(screen.queryByRole("heading", { name: "Key points" })).not.toBeInTheDocument();
  expect(document.querySelector("video")).toBeNull();
});

test("nothing is selected: the detail says to select a recording", async () => {
  events = stubMeetingsApi(rows);
  store = new MeetingsStore();
  render(Detail, { store });
  expect(screen.getByText("Select a recording")).toBeInTheDocument();
});

test("the detail is Vietnamese in Vietnamese", async () => {
  setLanguage("vi");
  const { user } = await opened();
  setLanguage("vi");

  expect(await screen.findByRole("button", { name: "Copy tóm tắt" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Thêm: tóm tắt lại, mở thư mục, xem log, xoá" }));
  expect(screen.getByRole("button", { name: "Xoá bản ghi (chuyển vào Thùng rác)" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Xuất" })).toBeInTheDocument();
});
