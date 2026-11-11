import { afterEach, describe, expect, test, vi } from "vitest";
import { fakeDetail, fakeError, fakeJob, fakeRecording, fakeRow, IDLE_RECORDING, stubMeetingsApi } from "../test-helpers";
import { api } from "./api";
import type { RecordingView } from "./api";
import { currentLanguage, setLanguage } from "./i18n";
import { MeetingsStore } from "./meetings-store.svelte";

let events: ReturnType<typeof stubMeetingsApi>;
let store: MeetingsStore;

const ID = "2026-03-02 10-00-00";
const OTHER = "2026-03-01 09-00-00";
const row = fakeRow({ id: ID });
const other = fakeRow({ id: OTHER, title: "Retro", created: new Date(2026, 2, 1, 9).getTime() / 1000 });

/** What the window shows of the row: the list with the jobs that were heard laid on it. */
const shown = (id: string) => store.items.find((item) => item.id === id);

async function started(rows = [row, other]): Promise<void> {
  events = stubMeetingsApi(rows);
  store = new MeetingsStore();
  await store.start();
}

afterEach(() => {
  store?.stop();
  vi.useRealTimers();
  vi.restoreAllMocks();
  setLanguage("en");
});

describe("loading", () => {
  test("lists the recordings and asks what is ready when it starts, and says when it has", async () => {
    events = stubMeetingsApi([row, other]);
    store = new MeetingsStore();
    expect(store.loaded).toBe(false);

    await store.start();

    expect(api.meetingsList).toHaveBeenCalledWith("");
    expect(store.rows.map((r) => r.id)).toEqual([ID, OTHER]);
    expect(store.readiness).toEqual({ ai: true, whisper: true, language: "en" });
    expect(store.loaded).toBe(true);
    // Nothing is selected for the person: "Select a recording" is the first thing the detail says.
    expect(store.selectedId).toBeNull();
  });

  test("a list that can't be read says why and is tried again when something changes", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.meetingsList).mockRejectedValueOnce("The library could not be read.");
    store = new MeetingsStore();
    await store.start();
    expect(store.notice).toBe("The library could not be read.");
    expect(store.loaded).toBe(false);

    events.emitChanged();
    await vi.waitFor(() => expect(store.rows).toHaveLength(1));
    expect(store.loaded).toBe(true);
  });

  test("an older answer of the list never replaces a newer one", async () => {
    await started();
    let answerOld: (rows: typeof store.rows) => void = () => {};
    vi.mocked(api.meetingsList)
      .mockImplementationOnce(() => new Promise((resolve) => (answerOld = resolve)))
      .mockResolvedValueOnce([other]);

    const old = store.load();
    await store.load();
    answerOld([row, other]);
    await old;

    expect(store.rows.map((r) => r.id)).toEqual([OTHER]);
  });

  test("the readiness is asked again when the window gets the focus back, as the keys are set in another window", async () => {
    await started();
    vi.mocked(api.meetingsReadiness).mockResolvedValue({ ai: false, whisper: true, language: "en" });

    window.dispatchEvent(new Event("focus"));

    await vi.waitFor(() => expect(store.readiness).toEqual({ ai: false, whisper: true, language: "en" }));
  });

  test("the window speaks the language that readiness tells, from the start", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.meetingsReadiness).mockResolvedValue({ ai: true, whisper: true, language: "vi" });
    store = new MeetingsStore();

    await store.start();

    expect(currentLanguage()).toBe("vi");
    expect(document.documentElement.lang).toBe("vi");
  });

  test("a change of the language in Settings reaches the window when it gets the focus back", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.meetingsReadiness).mockResolvedValue({ ai: true, whisper: true, language: "en" });
    store = new MeetingsStore();
    await store.start();
    expect(currentLanguage()).toBe("en");

    vi.mocked(api.meetingsReadiness).mockResolvedValue({ ai: true, whisper: true, language: "vi" });
    window.dispatchEvent(new Event("focus"));
    await vi.waitFor(() => expect(currentLanguage()).toBe("vi"));

    vi.mocked(api.meetingsReadiness).mockResolvedValue({ ai: true, whisper: true, language: "en" });
    window.dispatchEvent(new Event("focus"));
    await vi.waitFor(() => expect(currentLanguage()).toBe("en"));
  });

  test("a readiness that can't be read leaves the language as it was and says why", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.meetingsReadiness).mockRejectedValue("The settings could not be read.");
    store = new MeetingsStore();

    await store.start();

    expect(currentLanguage()).toBe("en");
    expect(store.notice).toBe("The settings could not be read.");
  });

  test("a listener that can't be set up is said, the list is read all the same, and the other listener is still stopped with the store", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.onMeetingJob).mockRejectedValue("The events are not available.");
    store = new MeetingsStore();

    await store.start();

    expect(store.notice).toBe("The events are not available.");
    expect(store.rows).toHaveLength(1);
    expect(events.listening().changed).toBe(1);
    store.stop();
    expect(events.listening().changed).toBe(0);
  });

  test("when it stops, it hears nothing more", async () => {
    await started();
    expect(events.listening()).toEqual({ jobs: 1, changed: 1, drops: 0 });

    store.stop();

    expect(events.listening()).toEqual({ jobs: 0, changed: 0, drops: 0 });
  });

  test("a stop that comes before the listeners are ready stops them as soon as they are", async () => {
    events = stubMeetingsApi([row]);
    store = new MeetingsStore();

    const starting = store.start();
    store.stop();
    await starting;

    expect(events.listening()).toEqual({ jobs: 0, changed: 0, drops: 0 });
  });
});

describe("search", () => {
  test("search_debounces_and_calls_list: the list is asked once, 200 ms after the last key, with what was typed", async () => {
    await started();
    vi.mocked(api.meetingsList).mockClear();
    vi.useFakeTimers();

    store.search("p");
    await vi.advanceTimersByTimeAsync(150);
    store.search("pl");
    await vi.advanceTimersByTimeAsync(199);
    expect(api.meetingsList).not.toHaveBeenCalled();
    // The box shows what was typed without waiting for the list.
    expect(store.query).toBe("pl");

    await vi.advanceTimersByTimeAsync(1);

    expect(api.meetingsList).toHaveBeenCalledTimes(1);
    expect(api.meetingsList).toHaveBeenCalledWith("pl");
  });

  test("the list is what the Rust side answered for the query", async () => {
    await started();
    vi.mocked(api.meetingsList).mockResolvedValue([other]);
    vi.useFakeTimers();

    store.search("retro");
    await vi.advanceTimersByTimeAsync(200);

    expect(store.rows.map((r) => r.id)).toEqual([OTHER]);
  });

  test("a search that is waiting is dropped when the store stops", async () => {
    await started();
    vi.mocked(api.meetingsList).mockClear();
    vi.useFakeTimers();

    store.search("plan");
    store.stop();
    await vi.advanceTimersByTimeAsync(500);

    expect(api.meetingsList).not.toHaveBeenCalled();
  });
});

describe("jobs heard while they run", () => {
  test("meeting_job_event_updates_row_live: the row shows the step and the figure of each event, and how the job ended", async () => {
    await started();
    vi.mocked(api.meetingsList).mockClear();
    expect(shown(ID)?.live).toBeNull();

    events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.4 }));
    expect(shown(ID)?.live).toMatchObject({ step: "transcript", progress: 0.4 });

    events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.6 }));
    expect(shown(ID)?.live?.progress).toBe(0.6);

    const error = fakeError({ kind: "Api", key: "meetings.error.summary_failed", detail: "401" });
    events.emitJob(fakeJob({ id: ID, step: null, outcome: { kind: "failed", error } }));
    expect(shown(ID)?.live).toBeNull();
    expect(shown(ID)?.error).toEqual(error);

    // The other row is untouched, and no event of progress asked for the list: a search reads every summary and transcript.
    expect(shown(OTHER)).toEqual(other);
    expect(api.meetingsList).not.toHaveBeenCalled();
  });

  test("a job that is done leaves its row with neither a job nor an error", async () => {
    await started([fakeRow({ id: ID, live: fakeJob({ id: ID }) })]);
    expect(shown(ID)?.live).not.toBeNull();

    events.emitJob(fakeJob({ id: ID, step: null, outcome: { kind: "done", summary: true } }));

    expect(shown(ID)?.live).toBeNull();
    expect(shown(ID)?.error).toBeNull();
  });

  test("a job that has not begun (no step yet) is a live row all the same", async () => {
    await started();

    events.emitJob(fakeJob({ id: ID, step: null, progress: null }));

    expect(shown(ID)?.live).toMatchObject({ step: null });
  });

  test("the job a list carries shows too, and a newer event wins over it", async () => {
    await started([fakeRow({ id: ID, live: fakeJob({ id: ID, step: "audio", progress: 0.1 }) })]);
    expect(shown(ID)?.live).toMatchObject({ step: "audio", progress: 0.1 });

    events.emitJob(fakeJob({ id: ID, step: "audio", progress: 0.5 }));

    expect(shown(ID)?.live?.progress).toBe(0.5);
  });

  test("an event of a recording the list does not have yet shows as soon as the list has it", async () => {
    await started([other]);

    events.emitJob(fakeJob({ id: ID, step: "download", progress: 0.2 }));
    expect(shown(ID)).toBeUndefined();

    vi.mocked(api.meetingsList).mockResolvedValue([row, other]);
    await store.load();

    expect(shown(ID)?.live).toMatchObject({ step: "download", progress: 0.2 });
  });

  test("a change of the list brings the list again and forgets what was heard of the jobs: a delete may skip the end of a job", async () => {
    await started();
    events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.4 }));
    vi.mocked(api.meetingsList).mockClear();
    vi.mocked(api.meetingsList).mockResolvedValue([fakeRow({ id: ID, title: "Renamed" }), other]);

    events.emitChanged();

    await vi.waitFor(() => expect(shown(ID)?.title).toBe("Renamed"));
    expect(shown(ID)?.live).toBeNull();
    expect(api.meetingsList).toHaveBeenCalledTimes(1);
  });

  test("the list asked for after a change keeps the query that is typed", async () => {
    await started();
    vi.useFakeTimers();
    store.search("plan");
    await vi.advanceTimersByTimeAsync(200);
    vi.useRealTimers();
    vi.mocked(api.meetingsList).mockClear();

    events.emitChanged();

    await vi.waitFor(() => expect(api.meetingsList).toHaveBeenCalledWith("plan"));
  });
});

describe("the selected recording", () => {
  test("selecting one loads its detail, and shows the rest of the page of the one before no more", async () => {
    await started();

    await store.select(ID);
    expect(store.selectedId).toBe(ID);
    expect(store.detail?.row.id).toBe(ID);

    const answering = store.select(OTHER);
    // The detail of the one before is not the detail of this one while it is on its way.
    expect(store.detail).toBeNull();
    await answering;
    expect(store.detail?.row.id).toBe(OTHER);
  });

  test("the answer for a recording that was selected before is dropped", async () => {
    await started();
    let answerFirst: (detail: ReturnType<typeof fakeDetail>) => void = () => {};
    vi.mocked(api.meetingDetail).mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)));

    const first = store.select(ID);
    await store.select(OTHER);
    answerFirst(fakeDetail({ row }));
    await first;

    expect(store.selectedId).toBe(OTHER);
    expect(store.detail?.row.id).toBe(OTHER);
  });

  test("a detail that can't be read says why and leaves nothing selected", async () => {
    await started();
    vi.mocked(api.meetingDetail).mockRejectedValue("The recording was not found.");

    await store.select(ID);

    expect(store.notice).toBe("The recording was not found.");
    expect(store.selectedId).toBeNull();
    expect(store.detail).toBeNull();
  });

  test("the detail is read again when the step of its job changes, so that a transcript that was just written shows, and not for a figure of progress", async () => {
    await started([fakeRow({ id: ID, live: fakeJob({ id: ID, step: "audio" }) }), other]);
    await store.select(ID);
    vi.mocked(api.meetingDetail).mockClear();

    events.emitJob(fakeJob({ id: ID, step: "audio", progress: 0.5 }));
    await Promise.resolve();
    expect(api.meetingDetail).not.toHaveBeenCalled();

    events.emitJob(fakeJob({ id: ID, step: "transcript" }));
    await vi.waitFor(() => expect(api.meetingDetail).toHaveBeenCalledTimes(1));
    expect(api.meetingDetail).toHaveBeenCalledWith(ID);

    events.emitJob(fakeJob({ id: ID, step: "transcript", progress: 0.2 }));
    await Promise.resolve();
    expect(api.meetingDetail).toHaveBeenCalledTimes(1);
  });

  test("the detail is read again when the job ends, and not for the job of another recording", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingDetail).mockClear();

    events.emitJob(fakeJob({ id: OTHER, step: "summary" }));
    await Promise.resolve();
    expect(api.meetingDetail).not.toHaveBeenCalled();

    events.emitJob(fakeJob({ id: ID, step: null, outcome: { kind: "done", summary: false } }));
    await vi.waitFor(() => expect(api.meetingDetail).toHaveBeenCalledWith(ID));
  });

  test("a change of the list reads the detail again too, as a title may have changed", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingDetail).mockClear();

    events.emitChanged();

    await vi.waitFor(() => expect(api.meetingDetail).toHaveBeenCalledWith(ID));
  });

  test("the recording that was selected and is gone (deleted) is replaced by the first of the list", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingsList).mockResolvedValue([other]);

    events.emitChanged();

    await vi.waitFor(() => expect(store.selectedId).toBe(OTHER));
    await vi.waitFor(() => expect(store.detail?.row.id).toBe(OTHER));
  });

  test("with nothing left in the library, nothing is selected", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingsList).mockResolvedValue([]);

    events.emitChanged();

    await vi.waitFor(() => expect(store.selectedId).toBeNull());
    expect(store.detail).toBeNull();
  });

  test("a recording that a search leaves out of the list stays selected", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingsList).mockResolvedValue([other]);
    vi.useFakeTimers();

    store.search("retro");
    await vi.advanceTimersByTimeAsync(200);

    expect(store.selectedId).toBe(ID);
  });

  test("a recording that was deleted while a search leaves it selected is let go of without a word when the library changes", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingsList).mockResolvedValue([other]);
    vi.useFakeTimers();
    store.search("retro");
    await vi.advanceTimersByTimeAsync(200);
    vi.useRealTimers();
    expect(store.selectedId).toBe(ID);
    vi.mocked(api.meetingDetail).mockRejectedValue("The recording was not found.");

    events.emitChanged();

    await vi.waitFor(() => expect(store.selectedId).toBeNull());
    expect(store.detail).toBeNull();
    expect(store.notice).toBe("");
  });

  test("a detail that can't be read again when a job moves on is let go of without a word too", async () => {
    await started([fakeRow({ id: ID, live: fakeJob({ id: ID, step: "audio" }) })]);
    await store.select(ID);
    vi.mocked(api.meetingDetail).mockRejectedValue("The recording was not found.");

    events.emitJob(fakeJob({ id: ID, step: "transcript" }));

    await vi.waitFor(() => expect(store.selectedId).toBeNull());
    expect(store.notice).toBe("");
  });

  test("the row of the selected recording is the one with the jobs laid on it, and before the list has it, the detail's", async () => {
    await started([other]);
    await store.select(ID);
    expect(store.selectedRow?.id).toBe(ID);

    events.emitJob(fakeJob({ id: ID, step: "download", progress: 0.3 }));

    expect(store.selectedRow?.live).toMatchObject({ step: "download", progress: 0.3 });
  });
});

describe("what a person asks the window to do", () => {
  test("a file that was picked is selected, and a dialog that was closed does nothing", async () => {
    await started();
    vi.mocked(api.meetingsPickFiles).mockResolvedValueOnce([]);
    await store.pickFiles();
    expect(store.selectedId).toBeNull();

    vi.mocked(api.meetingsPickFiles).mockResolvedValueOnce([ID, OTHER]);
    await store.pickFiles();

    expect(store.selectedId).toBe(ID);
  });

  test("files that were dropped are imported by their paths and the first is selected", async () => {
    await started();
    vi.mocked(api.meetingsImportFiles).mockResolvedValue([ID]);

    await store.importFiles(["/home/lan/a.mp4", "/home/lan/b.m4a"]);

    expect(api.meetingsImportFiles).toHaveBeenCalledWith(["/home/lan/a.mp4", "/home/lan/b.m4a"]);
    expect(store.selectedId).toBe(ID);
  });

  test("a drop with nothing to import says so", async () => {
    await started();
    vi.mocked(api.meetingsImportFiles).mockResolvedValue([]);

    await store.importFiles(["/home/lan/folder"]);

    expect(store.notice).toBe("Nothing to import: drop files, not folders.");
  });

  test("a drop of no paths calls nothing", async () => {
    await started();

    await store.importFiles([]);

    expect(api.meetingsImportFiles).not.toHaveBeenCalled();
  });

  test("a link is imported as it is typed, trimmed, and its recording is selected", async () => {
    await started();
    vi.mocked(api.meetingsImportLink).mockResolvedValue(ID);

    const failure = await store.importLink("  https://youtu.be/abc \n");

    expect(failure).toBeNull();
    expect(api.meetingsImportLink).toHaveBeenCalledWith("https://youtu.be/abc");
    expect(store.selectedId).toBe(ID);
  });

  test("a link that is refused gives the words of the error, and selects nothing", async () => {
    await started();
    vi.mocked(api.meetingsImportLink).mockRejectedValue(
      fakeError({ key: "meetings.error.invalid_link", detail: null }),
    );

    const failure = await store.importLink("not a link");

    expect(failure).toBe("Invalid link.");
    expect(store.selectedId).toBeNull();
  });

  test("Try again starts the job from the step that failed", async () => {
    await started();

    expect(await store.retry(ID)).toBe(true);

    expect(api.meetingRetry).toHaveBeenCalledWith(ID);
  });

  test("Retry summary asks for the summary only", async () => {
    await started();

    expect(await store.resummarize(ID)).toBe(true);

    expect(api.meetingResummarize).toHaveBeenCalledWith(ID);
    expect(api.meetingRetry).not.toHaveBeenCalled();
  });

  test("a recording that has a job already says so in a short notice, with no offer to try again", async () => {
    await started();
    vi.mocked(api.meetingRetry).mockRejectedValue(fakeError({ key: "meetings.error.busy", detail: null }));

    expect(await store.retry(ID)).toBe(false);

    expect(store.notice).toBe("This recording is being processed. Try again when it is done.");
  });

  test("an error of the Rust side that is only a sentence shows as it is", async () => {
    await started();
    vi.mocked(api.meetingResummarize).mockRejectedValue("The recording was not found.");

    await store.resummarize(ID);

    expect(store.notice).toBe("The recording was not found.");
  });

  test("Cancel asks the job to stop", async () => {
    await started();

    await store.cancel(ID);

    expect(api.meetingCancel).toHaveBeenCalledWith(ID);
  });

  test("a copy goes through the clipboard command, and one that fails says so", async () => {
    await started();
    expect(await store.copy("brew install yt-dlp")).toBe(true);
    expect(api.copyText).toHaveBeenCalledWith("brew install yt-dlp");

    vi.mocked(api.copyText).mockRejectedValue("The clipboard is busy.");
    expect(await store.copy("x")).toBe(false);
    expect(store.notice).toBe("The clipboard is busy.");
  });

  test("Settings is opened on the page it is asked for", async () => {
    await started();

    await store.openSettings("accounts");

    expect(api.meetingsOpenSettings).toHaveBeenCalledWith("accounts");
  });

  test("the notice goes away when it is dismissed", async () => {
    await started();
    store.notify("Something.");
    expect(store.notice).toBe("Something.");

    store.dismissNotice();

    expect(store.notice).toBe("");
  });

  test("the log is opened for a recording and closed", async () => {
    await started();
    expect(store.logFor).toBeNull();

    store.showLog(ID);
    expect(store.logFor).toBe(ID);

    store.closeLog();
    expect(store.logFor).toBeNull();
  });

  test("a dialog is open while the log is, or the link dialog is", async () => {
    await started();
    expect(store.modalOpen).toBe(false);

    store.showLog(ID);
    expect(store.modalOpen).toBe(true);
    store.closeLog();
    expect(store.modalOpen).toBe(false);

    store.openLinkDialog();
    expect(store.linkDialogOpen).toBe(true);
    expect(store.modalOpen).toBe(true);
    store.closeLinkDialog();
    expect(store.linkDialogOpen).toBe(false);
    expect(store.modalOpen).toBe(false);
  });

  test("a dialog is open while the speaker sheet is, so that a paste is not taken for the library", async () => {
    await started();

    store.openRename("Speaker 2");
    expect(store.modalOpen).toBe(true);
    store.closeRename();
    expect(store.modalOpen).toBe(false);

    // The sheet as a whole (no speaker named) is a dialog too.
    store.openRename();
    expect(store.modalOpen).toBe(true);
  });

  test("the list can be hidden and shown", async () => {
    await started();
    expect(store.sidebarOpen).toBe(true);

    store.toggleSidebar();
    expect(store.sidebarOpen).toBe(false);

    store.toggleSidebar();
    expect(store.sidebarOpen).toBe(true);
  });
});

describe("the questions asked", () => {
  test("are kept for each recording in memory, for the window to show again", async () => {
    await started();
    const asked = [{ question: "Who owns the docs?", answer: null, error: null }];

    store.asks.set(ID, asked);

    expect(store.asks.get(ID)).toBe(asked);
    expect(store.asks.get(OTHER)).toBeUndefined();
  });
});

describe("asking about a recording", () => {
  test("ask_submits_once: the question is kept with a waiting answer, and the answer takes its place", async () => {
    await started();
    let answer: (text: string) => void = () => {};
    vi.mocked(api.meetingAsk).mockImplementation(() => new Promise((resolve) => (answer = resolve)));

    const done = store.ask(ID, "  Who owns the docs?  ");
    expect(api.meetingAsk).toHaveBeenCalledWith(ID, "Who owns the docs?");
    expect(store.asks.get(ID)).toEqual([{ question: "Who owns the docs?", answer: null, error: null }]);
    expect(store.asking(ID)).toBe(true);
    expect(store.asking(OTHER)).toBe(false);

    answer("An does, see [01:05].");
    await done;

    expect(store.asks.get(ID)).toEqual([{ question: "Who owns the docs?", answer: "An does, see [01:05].", error: null }]);
    expect(store.asking(ID)).toBe(false);
  });

  test("a question is asked once while its answer is awaited, and an empty one is not asked", async () => {
    await started();
    vi.mocked(api.meetingAsk).mockImplementation(() => new Promise(() => {}));

    void store.ask(ID, "First?");
    void store.ask(ID, "Second?");
    void store.ask(OTHER, "   ");

    expect(api.meetingAsk).toHaveBeenCalledTimes(1);
    expect(store.asks.get(ID)?.map((qa) => qa.question)).toEqual(["First?"]);
    expect(store.asks.has(OTHER)).toBe(false);
  });

  test("ask_error_shows_message: a refusal is kept with the question, as a view that the window words", async () => {
    await started();
    const refusal = fakeError({ kind: "Api", key: "meetings.error.no_ai", detail: null });
    vi.mocked(api.meetingAsk).mockRejectedValue(refusal);

    await store.ask(ID, "Who?");

    expect(store.asks.get(ID)).toEqual([{ question: "Who?", answer: null, error: refusal }]);
    expect(store.asking(ID)).toBe(false);
  });

  test("a rejection that is a sentence is kept as an error of no kind in particular", async () => {
    await started();
    vi.mocked(api.meetingAsk).mockRejectedValue("The IPC failed.");

    await store.ask(ID, "Who?");

    expect(store.asks.get(ID)?.[0].error).toEqual({
      kind: "Other",
      key: "meetings.error.other",
      detail: "The IPC failed.",
      command: null,
    });
  });

  test("questions of a recording come one after the other, and another recording has its own", async () => {
    await started();
    vi.mocked(api.meetingAsk).mockResolvedValueOnce("One.").mockResolvedValueOnce("Two.").mockResolvedValueOnce("Three.");

    await store.ask(ID, "a?");
    await store.ask(ID, "b?");
    await store.ask(OTHER, "c?");

    expect(store.asks.get(ID)?.map((qa) => [qa.question, qa.answer])).toEqual([
      ["a?", "One."],
      ["b?", "Two."],
    ]);
    expect(store.asks.get(OTHER)?.map((qa) => qa.answer)).toEqual(["Three."]);
  });
});

describe("what the detail's menus do", () => {
  test("a task is ticked with the index of the action item, and a failure says why and gives false", async () => {
    await started();
    expect(await store.setTask(ID, 2, true)).toBe(true);
    expect(api.meetingSetTask).toHaveBeenCalledWith(ID, 2, true);

    vi.mocked(api.meetingSetTask).mockRejectedValue("Could not write tasks.json.");
    expect(await store.setTask(ID, 2, false)).toBe(false);
    expect(store.notice).toBe("Could not write tasks.json.");
  });

  test("the folder is opened, and a failure is a notice", async () => {
    await started();
    await store.openFolder(ID);
    expect(api.meetingOpenFolder).toHaveBeenCalledWith(ID);

    vi.mocked(api.meetingOpenFolder).mockRejectedValue("Could not open the folder.");
    await store.openFolder(ID);
    expect(store.notice).toBe("Could not open the folder.");
  });

  test("the media is opened in another app, and a view that says why not is worded", async () => {
    await started();
    await store.openMedia(ID);
    expect(api.meetingOpenMedia).toHaveBeenCalledWith(ID);
    expect(store.notice).toBe("");

    vi.mocked(api.meetingOpenMedia).mockRejectedValue(fakeError({ kind: "NoMedia", key: "meetings.error.no_media", detail: null }));
    await store.openMedia(ID);
    expect(store.notice).toBe("No media file.");
  });

  test("export_menu_calls_commands: the file is saved with the command, and a closed dialog is no failure", async () => {
    await started();
    vi.mocked(api.meetingExport).mockResolvedValueOnce(false);
    expect(await store.exportMarkdown(ID)).toBe(false);
    expect(store.notice).toBe("");

    expect(await store.exportMarkdown(ID)).toBe(true);
    expect(api.meetingExport).toHaveBeenCalledWith(ID);
  });

  test("a failed export says why, in the words of the view", async () => {
    await started();
    vi.mocked(api.meetingExport).mockRejectedValue(fakeError({ detail: "Could not write the file: disk full" }));

    expect(await store.exportMarkdown(ID)).toBe(false);

    expect(store.notice).toBe("Could not write the file: disk full");
  });

  test("the whole Markdown is read and copied", async () => {
    await started();
    vi.mocked(api.meetingMarkdown).mockResolvedValue("# Weekly planning\n\n---\n\n## Transcript");

    expect(await store.copyMarkdown(ID)).toBe(true);

    expect(api.meetingMarkdown).toHaveBeenCalledWith(ID);
    expect(api.copyText).toHaveBeenCalledWith("# Weekly planning\n\n---\n\n## Transcript");
  });

  test("a Markdown that can't be read is not copied, and says why", async () => {
    await started();
    vi.mocked(api.meetingMarkdown).mockRejectedValue("Recording not found.");

    expect(await store.copyMarkdown(ID)).toBe(false);

    expect(api.copyText).not.toHaveBeenCalled();
    expect(store.notice).toBe("Recording not found.");
  });

  test("the rename of speakers is asked for, for one speaker or for all, and is forgotten when another recording is selected", async () => {
    await started();
    await store.select(ID);
    expect(store.renameFor).toBeNull();

    store.openRename("Speaker 2");
    expect(store.renameFor).toEqual({ speaker: "Speaker 2" });
    store.openRename();
    expect(store.renameFor).toEqual({ speaker: null });

    await store.select(OTHER);
    expect(store.renameFor).toBeNull();

    store.openRename();
    store.closeRename();
    expect(store.renameFor).toBeNull();
  });

  test("the speakers get their new names with the choice to summarize again, and null says that it was done", async () => {
    await started();

    const failure = await store.renameSpeakers(ID, { "Speaker 1": "An" }, true);

    expect(failure).toBeNull();
    expect(api.meetingRenameSpeakers).toHaveBeenCalledWith(ID, { "Speaker 1": "An" }, true);
    expect(store.notice).toBe("");
  });

  test("a rename that was refused is given back as the view that says why, and is not a notice: the sheet shows it", async () => {
    await started();
    const busy = fakeError({ key: "meetings.error.busy", detail: null });
    vi.mocked(api.meetingRenameSpeakers).mockRejectedValueOnce(busy);

    expect(await store.renameSpeakers(ID, { "Speaker 1": "An" }, false)).toEqual(busy);
    expect(store.notice).toBe("");

    // Anything else that a failed call is made of is an error of no kind in particular.
    vi.mocked(api.meetingRenameSpeakers).mockRejectedValueOnce("The IPC is gone.");
    expect(await store.renameSpeakers(ID, { "Speaker 1": "An" }, false)).toEqual(
      fakeError({ detail: "The IPC is gone." }),
    );
  });
});

describe("delete", () => {
  test("the player is released first, then the recording goes to the Trash, and the first row is selected", async () => {
    await started();
    await store.select(OTHER);
    const order: string[] = [];
    vi.mocked(api.meetingDelete).mockImplementation(async () => {
      order.push("delete");
    });
    // The library as it is after the delete.
    vi.mocked(api.meetingsList).mockResolvedValue([row]);

    expect(
      await store.remove(OTHER, () => {
        order.push("release");
      }),
    ).toBe(true);

    expect(order).toEqual(["release", "delete"]);
    expect(api.meetingDelete).toHaveBeenCalledWith(OTHER);
    expect(store.selectedId).toBe(ID);
  });

  test("the first row is selected when a search has left the deleted one in the list's place, too", async () => {
    await started();
    store.search("planning");
    await vi.waitFor(() => expect(api.meetingsList).toHaveBeenCalledWith("planning"));
    await store.select(ID);
    // After the delete the search finds nothing but another one.
    vi.mocked(api.meetingsList).mockResolvedValue([other]);

    await store.remove(ID);

    expect(store.selectedId).toBe(OTHER);
  });

  test("the last recording that goes leaves nothing selected", async () => {
    await started([row]);
    await store.select(ID);
    vi.mocked(api.meetingsList).mockResolvedValue([]);

    await store.remove(ID);

    expect(store.selectedId).toBeNull();
    expect(store.detail).toBeNull();
  });

  test("its questions are forgotten with it", async () => {
    await started();
    await store.ask(ID, "Who?");
    vi.mocked(api.meetingsList).mockResolvedValue([other]);

    await store.remove(ID);

    expect(store.asks.has(ID)).toBe(false);
  });

  test("a failure shows the detail of the view, keeps the recording selected, gives the player back and reads the recording again", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingDelete).mockRejectedValue("Could not move the recording to the Trash: in use");
    const before = store.detail;
    const restore = vi.fn();

    expect(await store.remove(ID, () => restore)).toBe(false);

    expect(store.notice).toBe("Could not move the recording to the Trash: in use");
    expect(store.selectedId).toBe(ID);
    expect(restore).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(store.detail).not.toBe(before));
    expect(store.detail?.row.id).toBe(ID);
  });

  test("a delete that works does not give the player back", async () => {
    await started();
    await store.select(ID);
    const restore = vi.fn();
    vi.mocked(api.meetingsList).mockResolvedValue([other]);

    await store.remove(ID, () => restore);

    expect(restore).not.toHaveBeenCalled();
  });

  test("a failure that is an error view is worded", async () => {
    await started();
    await store.select(ID);
    vi.mocked(api.meetingDelete).mockRejectedValue(fakeError({ detail: "Trash is full" }));

    await store.remove(ID);

    expect(store.notice).toBe("Trash is full");
  });
});

describe("the recording", () => {
  const RECORDING_ID = "2026-03-02 11-00-00";
  const live = fakeRow({ id: RECORDING_ID, title: "Screen recording 11:00 02/03", kind: "screen", duration: null, hasSummary: false, openTasks: 0 });

  test("it asks how the recording is when it starts, and hears each state after", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.recordingState).mockResolvedValue(fakeRecording({ phase: "paused", elapsedMs: 61_000 }));
    store = new MeetingsStore();
    expect(store.recording.phase).toBe("idle");

    await store.start();

    expect(store.recording).toMatchObject({ phase: "paused", elapsedMs: 61_000 });
    expect(store.isRecording).toBe(true);
    events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 62_000 }));
    expect(store.recording).toMatchObject({ phase: "recording", elapsedMs: 62_000 });
    events.sendRecordingState(IDLE_RECORDING);
    expect(store.isRecording).toBe(false);
  });

  test("a state that came by the event while the answer was on its way is newer than the answer", async () => {
    events = stubMeetingsApi([row]);
    let answer: (view: RecordingView) => void = () => {};
    vi.mocked(api.recordingState).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    store = new MeetingsStore();
    const starting = store.start();
    await vi.waitFor(() => expect(api.recordingState).toHaveBeenCalled());

    events.sendRecordingState(fakeRecording({ phase: "recording", elapsedMs: 5_000 }));
    answer(IDLE_RECORDING);
    await starting;

    expect(store.recording).toMatchObject({ phase: "recording", elapsedMs: 5_000 });
  });

  test("a state that can't be read leaves the recording as idle and says why", async () => {
    events = stubMeetingsApi([row]);
    vi.mocked(api.recordingState).mockRejectedValue("The recorder did not answer.");
    store = new MeetingsStore();

    await store.start();

    expect(store.isRecording).toBe(false);
    expect(store.notice).toBe("The recorder did not answer.");
  });

  test("it hears the state of the recording until it stops", async () => {
    await started();
    expect(events.recordingListeners()).toEqual({ state: 1, open: 0 });

    store.stop();

    expect(events.recordingListeners()).toEqual({ state: 0, open: 0 });
  });

  test("the folder of the recording that goes on is not a row of the library until the recording is over", async () => {
    await started([row, live]);
    expect(store.items.map((item) => item.id)).toEqual([ID, RECORDING_ID]);

    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID }));
    expect(store.items.map((item) => item.id)).toEqual([ID]);
    events.sendRecordingState(fakeRecording({ phase: "paused", meetingId: RECORDING_ID }));
    expect(store.items.map((item) => item.id)).toEqual([ID]);
    events.sendRecordingState(fakeRecording({ phase: "finishing", meetingId: RECORDING_ID }));
    expect(store.items.map((item) => item.id)).toEqual([ID]);

    events.sendRecordingState(IDLE_RECORDING);
    expect(store.items.map((item) => item.id)).toEqual([ID, RECORDING_ID]);
  });

  test("the list is not worked out again at each report of the time of the recording", async () => {
    await started([row, live]);
    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID, elapsedMs: 50 }));
    const items = store.items;

    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID, elapsedMs: 100 }));
    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID, elapsedMs: 150 }));

    expect(store.items).toBe(items);
  });

  test("deleting the selected recording while one goes on selects the next recording of the library, not the folder of the one that goes on", async () => {
    // The recording that goes on is the newest row of the list.
    await started([live, row, other]);
    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID }));
    await store.select(ID);
    vi.mocked(api.meetingDetail).mockClear();
    vi.mocked(api.meetingsList).mockResolvedValue([live, other]);

    expect(await store.remove(ID)).toBe(true);

    expect(store.selectedId).toBe(OTHER);
    expect(store.selectedRow?.id).toBe(OTHER);
    expect(api.meetingDetail).not.toHaveBeenCalledWith(RECORDING_ID);
  });

  test("a selected recording that is gone from the list is replaced by the first one that is shown, not by the one that goes on", async () => {
    await started([live, row, other]);
    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID }));
    await store.select(ID);
    vi.mocked(api.meetingDetail).mockClear();
    // The recording was deleted elsewhere (the folder, or the other window): the change of the library brings the list again.
    vi.mocked(api.meetingsList).mockResolvedValue([live, other]);

    await store.load();

    expect(store.selectedId).toBe(OTHER);
    expect(api.meetingDetail).not.toHaveBeenCalledWith(RECORDING_ID);
  });

  test("a library with nothing in it but the recording that goes on selects nothing", async () => {
    await started([live, row]);
    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID }));
    await store.select(ID);
    vi.mocked(api.meetingsList).mockResolvedValue([live]);

    await store.remove(ID);

    expect(store.selectedId).toBeNull();
    expect(store.selectedRow).toBeNull();
  });

  test("the recording that goes on can't be selected, and is when it is over", async () => {
    await started([live, row]);
    await store.select(ID);
    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID }));
    vi.mocked(api.meetingDetail).mockClear();

    await store.select(RECORDING_ID);

    expect(store.selectedId).toBe(ID);
    expect(api.meetingDetail).not.toHaveBeenCalled();

    events.sendRecordingState(IDLE_RECORDING);
    await store.select(RECORDING_ID);
    expect(store.selectedId).toBe(RECORDING_ID);
  });

  test("a recording that begins while its folder is the selected one lets go of it", async () => {
    await started([live, row]);
    await store.select(RECORDING_ID);
    expect(store.selectedId).toBe(RECORDING_ID);

    events.sendRecordingState(fakeRecording({ phase: "recording", meetingId: RECORDING_ID }));

    await vi.waitFor(() => expect(store.selectedId).toBe(ID));
    expect(store.selectedRow?.id).toBe(ID);
  });

  test("a recording that has no folder yet hides nothing", async () => {
    await started([row, live]);

    events.sendRecordingState(fakeRecording({ phase: "starting", meetingId: null }));

    expect(store.items).toHaveLength(2);
  });

  test("the picker is open from an ask, with the error of the ask, and an ask again changes only the error", () => {
    store = new MeetingsStore();
    expect(store.recorder).toBeNull();

    store.openRecorder();
    expect(store.recorder).toEqual({ error: null });
    const error = fakeError({ key: "recording.error.needs_ffmpeg", detail: null });
    store.openRecorder(error);
    expect(store.recorder).toEqual({ error });
    // Another ask with no error leaves the one that is shown: it is not answered yet.
    store.openRecorder();
    expect(store.recorder).toEqual({ error });

    store.closeRecorder();
    expect(store.recorder).toBeNull();
  });

  test("what a start said is the error of the picker that is open, and a picker that is not open keeps none", () => {
    store = new MeetingsStore();
    store.setRecorderError(fakeError());
    expect(store.recorder).toBeNull();

    store.openRecorder();
    const error = fakeError({ key: "recording.error.source" });
    store.setRecorderError(error);
    expect(store.recorder).toEqual({ error });
    store.setRecorderError(null);
    expect(store.recorder).toEqual({ error: null });
  });

  test("the picker is a dialog over the window: what is pasted or dropped is not for the library", () => {
    store = new MeetingsStore();

    store.openRecorder();

    expect(store.modalOpen).toBe(true);
    store.closeRecorder();
    expect(store.modalOpen).toBe(false);
  });

  test("the recording is stopped from the toolbar, and a stop that fails says why", async () => {
    await started();

    await store.stopRecording();
    expect(api.recordingStop).toHaveBeenCalledOnce();
    expect(store.notice).toBe("");

    vi.mocked(api.recordingStop).mockRejectedValue(
      fakeError({ key: "recording.error.stop_failed", detail: "disk full" }),
    );
    await store.stopRecording();
    expect(store.notice).toContain("Couldn't finish the recording: disk full");
  });

  test("System Settings is opened on the pane of Screen Recording, and a failure says why", async () => {
    await started();
    vi.spyOn(api, "openPermissionSettings").mockResolvedValue();

    await store.openScreenPermission();
    expect(api.openPermissionSettings).toHaveBeenCalledExactlyOnceWith("screen");

    vi.mocked(api.openPermissionSettings).mockRejectedValue("Could not open System Settings");
    await store.openScreenPermission();
    expect(store.notice).toBe("Could not open System Settings");
  });
});
