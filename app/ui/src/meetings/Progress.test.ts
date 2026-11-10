import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { JobView, MeetingDetail } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeDetail, fakeJob, stubMeetingsApi } from "../test-helpers";
import Progress from "./Progress.svelte";

const ID = "2026-03-02 10-00-00";
let store: MeetingsStore;

beforeEach(() => {
  stubMeetingsApi();
  store = new MeetingsStore();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  setLanguage("en");
});

function panel(job: JobView, kind: "file" | "link" | "screen" = "file", detail: MeetingDetail | null = null) {
  return render(Progress, { store, job, kind, detail });
}

/** The step cards: their names, the state each is in, and what each says of it. */
function cards() {
  return screen.getAllByRole("listitem").map((item) => ({
    text: item.textContent?.replace(/\s+/g, " ").trim(),
    state: item.getAttribute("data-state"),
  }));
}

test("progress_panel_shows_steps_and_time_left: the cards of a link, how long the step has run, and how long it has left, every second", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 2, 2, 10, 0, 30));
  const startedAt = Date.now() - 30_000;
  panel(fakeJob({ id: ID, step: "transcript", progress: 0.25, stepStartedMs: startedAt }), "link");

  expect(cards()).toEqual([
    { text: "Download ✓ done", state: "done" },
    { text: "Extract audio ✓ done", state: "done" },
    { text: "Transcript 25%", state: "active" },
    { text: "Summary waiting", state: "waiting" },
  ]);
  // A quarter done after 30 s: 90 s to go.
  expect(screen.getByText("Transcribing: running for 00:30 · about 01:30 left")).toBeInTheDocument();

  await vi.advanceTimersByTimeAsync(1000);
  await tick();

  expect(screen.getByText("Transcribing: running for 00:31 · about 01:33 left")).toBeInTheDocument();
});

test("a step with no figure yet says that it is running, and has no time left to tell", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 2, 2, 10, 5, 0));
  panel(fakeJob({ step: "audio", progress: null, stepStartedMs: Date.now() - 65_000 }), "file");

  expect(cards()).toEqual([
    { text: "Extract audio running", state: "active" },
    { text: "Transcript waiting", state: "waiting" },
    { text: "Summary waiting", state: "waiting" },
  ]);
  expect(screen.getByText("Extract audio: running for 01:05")).toBeInTheDocument();
});

test("the first 3 per cent of a step give no time left, as a guess from so little would swing", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 2, 2, 10, 5, 0));
  panel(fakeJob({ step: "transcript", progress: 0.02, stepStartedMs: Date.now() - 10_000 }));

  expect(screen.getByText("Transcribing: running for 00:10")).toBeInTheDocument();
});

test("a file has no Download card, and a step is a bar with its percent", () => {
  panel(fakeJob({ step: "summary", progress: 0.5, stepStartedMs: Date.now() }), "file");

  expect(cards().map((card) => card.state)).toEqual(["done", "done", "active"]);
  expect(screen.queryByText("Download")).not.toBeInTheDocument();
  expect(screen.getByRole("progressbar", { name: "Summary" })).toHaveAttribute("value", "50");
});

test("the card of a download is there for a job that is downloading, and the time is what the step has run", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 2, 2, 10, 0, 10));
  panel(fakeJob({ step: "download", progress: 0.5, stepStartedMs: Date.now() - 10_000 }), "link");

  expect(cards()[0]).toEqual({ text: "Download 50%", state: "active" });
  expect(screen.getByText("Downloading: running for 00:10 · about 00:10 left")).toBeInTheDocument();
});

test("a job that has not begun is waiting for its turn, with every card waiting: not a broken card", () => {
  panel(fakeJob({ step: null, progress: null }), "file");

  expect(cards().map((card) => card.state)).toEqual(["waiting", "waiting", "waiting"]);
  expect(screen.getByText("Waiting for its turn…")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
});

test("says that the window can be closed", () => {
  panel(fakeJob());

  expect(screen.getByText("You can close this window — Dictate will notify you when it's done.")).toBeInTheDocument();
});

test("cancel_button_calls_meeting_cancel", async () => {
  const user = userEvent.setup();
  panel(fakeJob({ id: ID }));

  await user.click(screen.getByRole("button", { name: "Cancel" }));

  expect(api.meetingCancel).toHaveBeenCalledWith(ID);
});

test("shows the transcript so far, line by line, below the status", () => {
  panel(fakeJob({ step: "summary" }), "file", fakeDetail());

  const preview = screen.getByRole("region", { name: "Transcript" });
  expect(within(preview).getByText("Let's start.")).toBeInTheDocument();
  expect(within(preview).getByText("Speaker 2")).toBeInTheDocument();
  expect(within(preview).getByText("01:05")).toBeInTheDocument();
});

test("the rougher live captions are called a draft", () => {
  panel(fakeJob({ step: "transcript" }), "screen", fakeDetail({ transcriptIsLive: true }));

  expect(screen.getByRole("region", { name: "Live transcript (draft — Whisper is making the accurate version)" })).toBeInTheDocument();
});

test("with no transcript yet there is no preview", () => {
  panel(fakeJob(), "file", fakeDetail({ transcript: [] }));
  expect(screen.queryByRole("region")).not.toBeInTheDocument();
});

test("a transcript that looks like HTML is shown as text", () => {
  const evil = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
  const { container } = panel(
    fakeJob(),
    "file",
    fakeDetail({ transcript: [{ start: 0, stamp: "00:00", speaker: '<b onmouseover="x">Eve</b>', text: evil }] }),
  );

  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("script")).toBeNull();
  expect(container.querySelector("b")).toBeNull();
  expect(screen.getByText(evil)).toBeInTheDocument();
});

test("the clock stops when the panel goes away", () => {
  vi.useFakeTimers();
  const { unmount } = panel(fakeJob());
  expect(vi.getTimerCount()).toBeGreaterThan(0);

  unmount();

  expect(vi.getTimerCount()).toBe(0);
});

test("speaks Vietnamese when the language is", () => {
  setLanguage("vi");
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 2, 2, 10, 0, 30));
  panel(fakeJob({ step: "transcript", progress: 0.25, stepStartedMs: Date.now() - 30_000 }), "file");

  expect(screen.getByText("Đang transcript: đã chạy 00:30 · còn khoảng 01:30")).toBeInTheDocument();
  expect(screen.getByText("Có thể đóng cửa sổ — Dictate sẽ báo khi xong.")).toBeInTheDocument();
});
