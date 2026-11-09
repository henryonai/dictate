import { afterEach, describe, expect, test, vi } from "vitest";
import { fakeDetail, fakeError, fakeJob, fakeRow, stubMeetingsApi } from "../test-helpers";
import { api } from "./api";
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
