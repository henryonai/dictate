import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { MeetingRow, Readiness } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakeDetail, fakeError, fakeJob, fakeRow, stubMeetingsApi } from "../test-helpers";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import MeetingsApp from "./MeetingsApp.svelte";

const ID = "2026-03-02 10-00-00";
const OTHER = "2026-03-01 09-00-00";
const today = new Date();
const at = (daysAgo: number, hour: number) =>
  new Date(today.getFullYear(), today.getMonth(), today.getDate() - daysAgo, hour).getTime() / 1000;

const rows: MeetingRow[] = [
  fakeRow({ id: ID, title: "Weekly planning", created: at(0, 8) }),
  fakeRow({ id: OTHER, title: "Retro", created: at(1, 15), kind: "link", openTasks: 0, duration: 3900 }),
];

let events: ReturnType<typeof stubMeetingsApi>;

/** Renders the window with these rows in the library, and waits until it has read the list. */
async function open(library: MeetingRow[] = rows, readiness?: Readiness) {
  events = stubMeetingsApi(library);
  if (readiness) events.setReadiness(readiness);
  const view = render(MeetingsApp);
  await vi.waitFor(() => expect(api.meetingsList).toHaveBeenCalled());
  await view.findByRole("toolbar");
  return view;
}

/** The text of the clipboard, as the web view would give it, for a paste. */
function paste(text: string, target: Element = document.body) {
  return fireEvent.paste(target, { clipboardData: { getData: () => text } });
}

beforeEach(() => {
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

// ---- The list

test("a library shows its recordings grouped by day, with no one selected, and the toolbar", async () => {
  await open();

  expect(await screen.findByRole("heading", { name: "Today" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Yesterday" })).toBeInTheDocument();
  const list = screen.getByRole("navigation", { name: "Recordings" });
  expect(within(list).getAllByRole("button").map((button) => button.textContent)).toEqual([
    expect.stringContaining("Weekly planning"),
    expect.stringContaining("Retro"),
  ]);
  expect(screen.getByRole("searchbox", { name: "Search summaries and transcripts" })).toBeInTheDocument();
  expect(screen.getByText("Select a recording")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Add file or link" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Settings" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Hide list" })).toBeInTheDocument();
});

test("a library with nothing in it shows the empty state, with no list and no search", async () => {
  await open([]);

  expect(await screen.findByRole("heading", { level: 1, name: "Record it, Dictate summarizes it" })).toBeInTheDocument();
  expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Hide list" })).not.toBeInTheDocument();
  // The toolbar still adds a file or a link.
  expect(screen.getByRole("button", { name: "Add file or link" })).toBeInTheDocument();
});

test("nothing is shown before the list has been read: an empty list is not yet an empty library", async () => {
  events = stubMeetingsApi(rows);
  vi.mocked(api.meetingsList).mockReturnValue(new Promise(() => {}));
  render(MeetingsApp);
  await vi.waitFor(() => expect(api.meetingsList).toHaveBeenCalled());

  expect(screen.queryByRole("heading", { level: 1 })).not.toBeInTheDocument();
  expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
});

test("the search asks for the list with what is typed, and a search that finds nothing says so, with the box still there", async () => {
  await open();
  vi.mocked(api.meetingsList).mockResolvedValue([]);

  await fireEvent.input(screen.getByRole("searchbox"), { target: { value: "zzz" } });

  expect(await screen.findByText("No results for “zzz”")).toBeInTheDocument();
  expect(api.meetingsList).toHaveBeenLastCalledWith("zzz");
  expect(screen.getByRole("searchbox")).toHaveValue("zzz");
  expect(screen.queryByRole("heading", { name: "Record it, Dictate summarizes it" })).not.toBeInTheDocument();
});

test("the list can be hidden and shown again", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(await screen.findByRole("button", { name: "Hide list" }));
  expect(screen.queryByRole("navigation", { name: "Recordings" })).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Show list" }));
  expect(screen.getByRole("navigation", { name: "Recordings" })).toBeInTheDocument();
});

test("a title that looks like HTML is shown as text, in the list and in the header", async () => {
  const user = userEvent.setup();
  const evil = '<img src=x onerror="alert(1)">';
  const { container } = await open([fakeRow({ id: ID, title: evil, created: at(0, 8) })]);

  await user.click(await screen.findByRole("button", { name: new RegExp(evil.replace(/[()]/g, "\\$&")) }));

  expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent(evil);
  expect(container.querySelector("img")).toBeNull();
});

// ---- The selected recording

test("a click on a recording shows its title, and where it came from, when and how long", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(await screen.findByRole("button", { name: /Weekly planning/ }));

  expect(await screen.findByRole("heading", { level: 1, name: "Weekly planning" })).toBeInTheDocument();
  expect(api.meetingDetail).toHaveBeenCalledWith(ID);
  expect(await screen.findByText(/^planning\.mp4 · .+ · 12 min$/)).toBeInTheDocument();
  expect(screen.queryByText("Select a recording")).not.toBeInTheDocument();
});

test("the header of a screen recording says so, in the window's language, and that of a link says its site", async () => {
  const user = userEvent.setup();
  const screenRow = fakeRow({ id: ID, title: "Standup", kind: "screen", created: at(0, 8) });
  const linkRow = fakeRow({ id: OTHER, title: "Talk", kind: "link", created: at(1, 15), duration: 3900 });
  await open([screenRow, linkRow]);
  vi.mocked(api.meetingDetail).mockImplementation(async (id) =>
    id === ID
      ? fakeDetail({ row: screenRow, sourceLabel: "", source: "screen" })
      : fakeDetail({ row: linkRow, sourceLabel: "youtube.com", source: "https://www.youtube.com/watch?v=x" }),
  );

  await user.click(await screen.findByRole("button", { name: /Standup/ }));
  expect(await screen.findByText(/^Screen recording · .+ · 12 min$/)).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: /Talk/ }));
  expect(await screen.findByText(/^youtube\.com · .+ · 1 h 5 min$/)).toBeInTheDocument();
});

test("a recording whose job is running shows the progress panel, and Cancel stops the job", async () => {
  const user = userEvent.setup();
  const busy = fakeRow({ id: ID, title: "Weekly planning", created: at(0, 8), live: fakeJob({ id: ID, step: "transcript", progress: 0.5, stepStartedMs: Date.now() }) });
  await open([busy]);

  await user.click(await screen.findByRole("button", { name: /Weekly planning/ }));
  expect(await screen.findByRole("list", { name: "Progress" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Cancel" }));

  expect(api.meetingCancel).toHaveBeenCalledWith(ID);
});

test("the progress panel follows the events of the job, and the error card takes its place when the job fails", async () => {
  const user = userEvent.setup();
  const busy = fakeRow({ id: ID, title: "Weekly planning", created: at(0, 8), live: fakeJob({ id: ID, step: "audio", stepStartedMs: Date.now() }) });
  await open([busy]);
  await user.click(await screen.findByRole("button", { name: /Weekly planning/ }));
  await screen.findByRole("list", { name: "Progress" });

  events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.4, stepStartedMs: Date.now() }));
  expect(await screen.findByText("40%")).toBeInTheDocument();

  events.emitJob(
    fakeJob({
      id: ID,
      step: null,
      outcome: { kind: "failed", error: fakeError({ kind: "Api", key: "meetings.error.summary_failed", detail: "401" }) },
    }),
  );
  expect(await screen.findByRole("heading", { name: "Couldn't summarize: API error" })).toBeInTheDocument();
  expect(screen.queryByRole("list", { name: "Progress" })).not.toBeInTheDocument();
});

test("a recording that failed shows the error card, and View log opens the log", async () => {
  const user = userEvent.setup();
  const failed = fakeRow({
    id: ID,
    title: "Weekly planning",
    created: at(0, 8),
    error: fakeError({ kind: "Other", key: "meetings.error.unfinished", detail: null }),
  });
  await open([failed]);
  vi.mocked(api.meetingLog).mockResolvedValue("a line of the log");
  await user.click(await screen.findByRole("button", { name: /Weekly planning/ }));

  expect(await screen.findByRole("heading", { name: "Something went wrong" })).toBeInTheDocument();
  expect(screen.getByText("Not finished processing")).toBeInTheDocument();
  // The transcript the detail has is kept.
  expect(screen.getByText(/The transcript is kept/)).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "View log" }));
  const sheet = await screen.findByRole("dialog", { name: "Log — Weekly planning" });
  expect(await within(sheet).findByText("a line of the log")).toBeInTheDocument();
});

// ---- Importing

test("paste_link_anywhere_imports: a link that is pasted with nothing focused is summarized", async () => {
  await open();

  await paste("  https://youtu.be/abc  ");

  await vi.waitFor(() => expect(api.meetingsImportLink).toHaveBeenCalledWith("https://youtu.be/abc"));
});

test("a paste that is not a link is left alone", async () => {
  await open();

  await paste("buy milk");
  await paste("ftp://example.com/x");

  expect(api.meetingsImportLink).not.toHaveBeenCalled();
});

test("a link pasted into a text field is text, and is not imported", async () => {
  await open();

  await paste("https://youtu.be/abc", screen.getByRole("searchbox"));

  expect(api.meetingsImportLink).not.toHaveBeenCalled();
});

test("a pasted link that is refused says why in a notice that can be dismissed", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.meetingsImportLink).mockRejectedValue(fakeError({ key: "meetings.error.invalid_link", detail: null }));

  await paste("http://");

  const notice = await screen.findByRole("alert");
  expect(notice).toHaveTextContent("Invalid link.");
  await user.click(within(notice).getByRole("button", { name: "Dismiss" }));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("drop_files_imports_paths: files that are dropped are imported by their paths, and their recording is selected", async () => {
  await open();
  vi.mocked(api.meetingsImportFiles).mockResolvedValue([ID]);

  events.emitDrop({ type: "drop", paths: ["/home/lan/talk.mp4", "/home/lan/call.m4a"] });

  await vi.waitFor(() =>
    expect(api.meetingsImportFiles).toHaveBeenCalledWith(["/home/lan/talk.mp4", "/home/lan/call.m4a"]),
  );
  expect(await screen.findByRole("heading", { level: 1, name: "Weekly planning" })).toBeInTheDocument();
});

test("while files are dragged over the window it says where to drop them, and it stops when they leave", async () => {
  await open();
  expect(screen.queryByText("Drop video or audio files here to summarize them")).not.toBeInTheDocument();

  events.emitDrop({ type: "enter" });
  expect(await screen.findByText("Drop video or audio files here to summarize them")).toBeInTheDocument();

  events.emitDrop({ type: "leave" });
  await vi.waitFor(() => expect(screen.queryByText("Drop video or audio files here to summarize them")).not.toBeInTheDocument());
});

test("a drop ends the message, and a drop of a folder says that there was nothing to import", async () => {
  await open();
  events.emitDrop({ type: "enter" });
  await screen.findByText("Drop video or audio files here to summarize them");

  events.emitDrop({ type: "drop", paths: ["/home/lan/folder"] });

  expect(await screen.findByRole("alert")).toHaveTextContent("Nothing to import: drop files, not folders.");
  expect(screen.queryByText("Drop video or audio files here to summarize them")).not.toBeInTheDocument();
});

test("the drop is heard until the window goes away", async () => {
  const view = await open();
  await vi.waitFor(() => expect(events.listening()).toEqual({ jobs: 1, changed: 1, drops: 1 }));

  view.unmount();

  await vi.waitFor(() => expect(events.listening()).toEqual({ jobs: 0, changed: 0, drops: 0 }));
});

test("the Add button opens a panel that offers a file and a link, and a file is chosen with the dialog of the system", async () => {
  const user = userEvent.setup();
  await open();
  const add = screen.getByRole("button", { name: "Add file or link" });
  expect(add).toHaveAttribute("aria-expanded", "false");

  await user.click(add);
  expect(add).toHaveAttribute("aria-expanded", "true");
  const menu = screen.getByRole("group", { name: "Add file or link" });
  expect(within(menu).getAllByRole("button").map((item) => item.textContent)).toEqual([
    "Choose video/audio file…",
    "Paste a link (YouTube, Vimeo…)…",
  ]);

  vi.mocked(api.meetingsPickFiles).mockResolvedValue([ID]);
  await user.click(within(menu).getByRole("button", { name: "Choose video/audio file…" }));

  expect(api.meetingsPickFiles).toHaveBeenCalled();
  expect(screen.queryByRole("group", { name: "Add file or link" })).not.toBeInTheDocument();
  expect(await screen.findByRole("heading", { level: 1, name: "Weekly planning" })).toBeInTheDocument();
});

test("the panel closes with Esc, and with a click anywhere else", async () => {
  const user = userEvent.setup();
  await open();
  const add = screen.getByRole("button", { name: "Add file or link" });

  await user.click(add);
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("group", { name: "Add file or link" })).not.toBeInTheDocument();
  expect(add).toHaveFocus();

  await user.click(add);
  await user.click(document.body);
  expect(screen.queryByRole("group", { name: "Add file or link" })).not.toBeInTheDocument();
});

test("the link dialog is filled with the link that is on the clipboard", async () => {
  const user = userEvent.setup();
  await open();
  await navigator.clipboard.writeText("https://vimeo.com/123");

  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));

  const dialog = await screen.findByRole("dialog", { name: "Summarize a video from a link" });
  expect(within(dialog).getByText("YouTube or any site yt-dlp supports.")).toBeInTheDocument();
  await vi.waitFor(() => expect(within(dialog).getByRole("textbox", { name: "Link" })).toHaveValue("https://vimeo.com/123"));
  expect(within(dialog).getByRole("textbox", { name: "Link" })).toHaveFocus();

  await user.click(within(dialog).getByRole("button", { name: "Summarize" }));

  expect(api.meetingsImportLink).toHaveBeenCalledWith("https://vimeo.com/123");
  await vi.waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
});

test("the link dialog starts empty when what is on the clipboard is not a link", async () => {
  const user = userEvent.setup();
  await open();
  await navigator.clipboard.writeText("buy milk");

  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));

  const field = await screen.findByRole("textbox", { name: "Link" });
  await Promise.resolve();
  expect(field).toHaveValue("");
});

test("the link dialog starts empty when the clipboard can't be read", async () => {
  const user = userEvent.setup();
  await open();
  vi.spyOn(navigator.clipboard, "readText").mockRejectedValue(new Error("denied"));

  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));

  expect(await screen.findByRole("textbox", { name: "Link" })).toHaveValue("");
});

test("Cancel and Esc close the link dialog without importing", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));
  await user.click(await screen.findByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));
  await screen.findByRole("dialog");
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(api.meetingsImportLink).not.toHaveBeenCalled();
});

test("a link that is refused leaves the dialog open, with the reason", async () => {
  const user = userEvent.setup();
  await open();
  vi.mocked(api.meetingsImportLink).mockRejectedValue(fakeError({ key: "meetings.error.invalid_link", detail: null }));

  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));
  await user.type(await screen.findByRole("textbox", { name: "Link" }), "nope{Enter}");

  expect(await screen.findByRole("alert")).toHaveTextContent("Invalid link.");
  expect(screen.getByRole("dialog")).toBeInTheDocument();
});

test("Settings opens the Settings window", async () => {
  const user = userEvent.setup();
  await open();

  await user.click(screen.getByRole("button", { name: "Settings" }));

  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("general");
});

// ---- Changes of the library

test("a change of the library brings the list again", async () => {
  await open();
  await screen.findByRole("button", { name: /Weekly planning/ });
  vi.mocked(api.meetingsList).mockResolvedValue([fakeRow({ id: OTHER, title: "Brand new", created: at(0, 9) })]);

  events.emitChanged();

  expect(await screen.findByRole("button", { name: /Brand new/ })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /Weekly planning/ })).not.toBeInTheDocument();
});

test("a job event moves the row of the list without asking for the list", async () => {
  await open();
  await screen.findByRole("button", { name: /Weekly planning/ });
  vi.mocked(api.meetingsList).mockClear();

  events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.3 }));

  expect(await screen.findByText("Transcribing · 30%")).toBeInTheDocument();
  expect(api.meetingsList).not.toHaveBeenCalled();
});

test("readiness_language_vi_makes_the_window_vietnamese: it speaks the language that the settings choose, and tells the page", async () => {
  await open(rows, { ai: true, whisper: true, language: "vi" });

  expect(await screen.findByText("Chọn một bản ghi")).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Hôm nay" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Thêm file hoặc link" })).toBeInTheDocument();
  expect(screen.getByRole("searchbox", { name: "Tìm trong tóm tắt và transcript" })).toBeInTheDocument();
  expect(document.documentElement.lang).toBe("vi");
});

test("it speaks English when the settings say so, whatever the language of the system is", async () => {
  vi.spyOn(navigator, "language", "get").mockReturnValue("vi-VN");

  await open(rows, { ai: true, whisper: true, language: "en" });

  expect(await screen.findByText("Select a recording")).toBeInTheDocument();
  expect(document.documentElement.lang).toBe("en");
});

test("a change of the language in Settings shows when the window gets the focus back", async () => {
  await open();
  expect(await screen.findByText("Select a recording")).toBeInTheDocument();

  events.setReadiness({ ai: true, whisper: true, language: "vi" });
  window.dispatchEvent(new Event("focus"));

  expect(await screen.findByText("Chọn một bản ghi")).toBeInTheDocument();
});

// ---- Things that go wrong, and dialogs

test("a window that can't hear the drop says so in a notice, and the list is there all the same", async () => {
  events = stubMeetingsApi(rows);
  vi.mocked(api.onFileDrop).mockRejectedValue("Dropping files is not available.");

  render(MeetingsApp);

  expect(await screen.findByRole("alert")).toHaveTextContent("Dropping files is not available.");
  expect(await screen.findByRole("button", { name: /Weekly planning/ })).toBeInTheDocument();
});

test("a start that fails says so in a notice, and not with a blank window", async () => {
  events = stubMeetingsApi(rows);
  vi.spyOn(MeetingsStore.prototype, "start").mockRejectedValue("The window could not start.");

  render(MeetingsApp);

  expect(await screen.findByRole("alert")).toHaveTextContent("The window could not start.");
});

test("a link pasted while the log is open is not imported", async () => {
  const user = userEvent.setup();
  const failed = fakeRow({ id: ID, title: "Weekly planning", created: at(0, 8), error: fakeError() });
  await open([failed]);
  await user.click(await screen.findByRole("button", { name: /Weekly planning/ }));
  await user.click(await screen.findByRole("button", { name: "View log" }));
  await screen.findByRole("dialog", { name: "Log — Weekly planning" });

  await paste("https://youtu.be/abc");

  expect(api.meetingsImportLink).not.toHaveBeenCalled();
});

test("a link pasted while the link dialog is open is not imported behind its back", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));
  await screen.findByRole("dialog");

  // Pasted on the dialog itself, not in its field.
  await paste("https://youtu.be/abc", screen.getByRole("dialog"));

  expect(api.meetingsImportLink).not.toHaveBeenCalled();
});

test("a link is imported again by a paste once the dialog is closed", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));
  await screen.findByRole("dialog");
  await user.keyboard("{Escape}");

  await paste("https://youtu.be/abc");

  await vi.waitFor(() => expect(api.meetingsImportLink).toHaveBeenCalledWith("https://youtu.be/abc"));
});

test("a press that starts in the link dialog and ends on the backdrop (a selection that was dragged) does not close it", async () => {
  const user = userEvent.setup();
  await open();
  await user.click(screen.getByRole("button", { name: "Add file or link" }));
  await user.click(screen.getByRole("button", { name: "Paste a link (YouTube, Vimeo…)…" }));
  const dialog = await screen.findByRole("dialog");
  const backdrop = dialog.parentElement as HTMLElement;

  await fireEvent.pointerDown(within(dialog).getByRole("textbox", { name: "Link" }));
  await fireEvent.click(backdrop);
  expect(screen.getByRole("dialog")).toBeInTheDocument();

  // A press that is on the backdrop from start to end closes it.
  await fireEvent.pointerDown(backdrop);
  await fireEvent.click(backdrop);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});
