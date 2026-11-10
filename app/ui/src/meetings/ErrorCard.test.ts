import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { ErrorView } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeError, stubMeetingsApi } from "../test-helpers";
import ErrorCard from "./ErrorCard.svelte";

const ID = "2026-03-02 10-00-00";
let store: MeetingsStore;

beforeEach(() => {
  stubMeetingsApi();
  store = new MeetingsStore();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

function card(error: ErrorView, hasTranscript = false) {
  return render(ErrorCard, { store, id: ID, error, hasTranscript });
}

const buttons = () => screen.getAllByRole("button").map((button) => button.textContent?.trim());

test("error_card_buttons_follow_kind: an API error offers the API settings and to retry the summary, and nothing else", async () => {
  const user = userEvent.setup();
  card(fakeError({ kind: "Api", key: "meetings.error.summary_failed", detail: "API error 401: bad key" }), true);

  expect(screen.getByRole("heading", { name: "Couldn't summarize: API error" })).toBeInTheDocument();
  expect(screen.getByText("Summary failed: API error 401: bad key")).toBeInTheDocument();
  expect(buttons()).toEqual(["Open API settings", "Retry summary", "View log"]);

  await user.click(screen.getByRole("button", { name: "Open API settings" }));
  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("accounts");

  await user.click(screen.getByRole("button", { name: "Retry summary" }));
  expect(api.meetingResummarize).toHaveBeenCalledWith(ID);
  expect(api.meetingRetry).not.toHaveBeenCalled();
});

test("a missing tool offers to copy the command that installs it, and to try again", async () => {
  const user = userEvent.setup();
  card(
    fakeError({ kind: "MissingTool", key: "meetings.error.ytdlp_missing", detail: null, command: "brew install yt-dlp ffmpeg" }),
  );

  expect(screen.getByRole("heading", { name: "Missing tool" })).toBeInTheDocument();
  expect(screen.getByText("yt-dlp isn't installed. Run: brew install yt-dlp ffmpeg")).toBeInTheDocument();
  expect(buttons()).toEqual(["Copy install command", "Try again", "View log"]);

  await user.click(screen.getByRole("button", { name: "Copy install command" }));
  expect(api.copyText).toHaveBeenCalledWith("brew install yt-dlp ffmpeg");
  // The button says that it worked.
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Try again" }));
  expect(api.meetingRetry).toHaveBeenCalledWith(ID);
});

test("a format that needs a tool is the same card, and without a command there is nothing to copy", () => {
  card(fakeError({ kind: "Unsupported", key: "meetings.error.needs_ffmpeg", detail: null, command: "brew install ffmpeg" }));
  expect(screen.getByRole("heading", { name: "Missing tool" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy install command" })).toBeInTheDocument();
});

test("a tool that is missing with no command to tell has no Copy button", () => {
  card(fakeError({ kind: "MissingTool", key: "meetings.error.ytdlp_missing", detail: null, command: null }));

  expect(buttons()).toEqual(["Try again", "View log"]);
});

test("a missing model offers Settings on the speech page, and to try again", async () => {
  const user = userEvent.setup();
  card(fakeError({ kind: "NoModel", key: "meetings.error.no_model", detail: null }));

  expect(screen.getByRole("heading", { name: "Something went wrong" })).toBeInTheDocument();
  expect(buttons()).toEqual(["Open Settings", "Try again", "View log"]);

  await user.click(screen.getByRole("button", { name: "Open Settings" }));
  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("speech");
});

test("a cancelled job says so, and can be tried again", () => {
  card(fakeError({ kind: "Cancelled", key: "meetings.error.cancelled", detail: null }), true);

  expect(screen.getByRole("heading", { name: "Cancelled" })).toBeInTheDocument();
  expect(buttons()).toEqual(["Try again", "View log"]);
  // Nothing was lost by a Cancel: it is not told that a transcript is kept as if something had gone wrong.
  expect(screen.queryByText(/transcript is kept/)).not.toBeInTheDocument();
});

test("any other error is something that went wrong, with Try again", async () => {
  const user = userEvent.setup();
  for (const kind of ["Empty", "NoAudio", "NoMedia", "Unfinished", "Other"] as const) {
    const view = card(fakeError({ kind, key: "meetings.error.other", detail: "Something broke." }));
    expect(screen.getByRole("heading", { name: "Something went wrong" })).toBeInTheDocument();
    expect(screen.getByText("Something broke.")).toBeInTheDocument();
    expect(buttons()).toEqual(["Try again", "View log"]);
    view.unmount();
  }

  card(fakeError());
  await user.click(screen.getByRole("button", { name: "Try again" }));
  expect(api.meetingRetry).toHaveBeenCalledWith(ID);
});

test("the transcript that was kept is said to be kept", () => {
  card(fakeError({ kind: "Api", key: "meetings.error.summary_failed" }), true);

  expect(screen.getByText("The transcript is kept — once fixed, just rerun the step that failed.")).toBeInTheDocument();
});

test("without a transcript there is nothing to say is kept", () => {
  card(fakeError({ kind: "Api", key: "meetings.error.summary_failed" }), false);

  expect(screen.queryByText(/transcript is kept/)).not.toBeInTheDocument();
});

test("View log opens the log of the recording", async () => {
  const user = userEvent.setup();
  card(fakeError());

  await user.click(screen.getByRole("button", { name: "View log" }));

  expect(store.logFor).toBe(ID);
});

test("a recording that has a job already says so in a notice, and the card stays as it was", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingRetry).mockRejectedValue(fakeError({ key: "meetings.error.busy", detail: null }));
  card(fakeError());

  await user.click(screen.getByRole("button", { name: "Try again" }));

  expect(store.notice).toBe("This recording is being processed. Try again when it is done.");
  expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
});

test("a message that looks like HTML is shown as text", () => {
  const { container } = card(fakeError({ detail: '<script>alert(1)</script><img src=x onerror="alert(2)">' }));

  expect(container.querySelector("script")).toBeNull();
  expect(container.querySelector("img")).toBeNull();
  expect(screen.getByText('<script>alert(1)</script><img src=x onerror="alert(2)">')).toBeInTheDocument();
});

test("speaks Vietnamese when the language is", () => {
  setLanguage("vi");
  card(fakeError({ kind: "MissingTool", key: "meetings.error.ytdlp_missing", detail: null, command: "brew install yt-dlp" }));

  expect(screen.getByRole("heading", { name: "Thiếu công cụ" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy lệnh cài" })).toBeInTheDocument();
});
