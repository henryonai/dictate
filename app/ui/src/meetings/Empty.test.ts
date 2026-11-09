import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeError, stubMeetingsApi } from "../test-helpers";
import Empty from "./Empty.svelte";

let store: MeetingsStore;

beforeEach(() => {
  stubMeetingsApi();
  store = new MeetingsStore();
  store.readiness = { ai: true, whisper: true, language: "en" };
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

/** The rows of "Ready?": the name of each and what it says of itself. */
function readyRows() {
  const section = screen.getByRole("region", { name: "Ready?" });
  return within(section)
    .getAllByRole("listitem")
    .map((item) => item.textContent?.replace(/\s+/g, " ").trim());
}

test("empty_library_shows_cards_and_ready_rows: the heading, the two cards, the drop zone and what is ready", () => {
  render(Empty, { store });

  expect(screen.getByRole("heading", { level: 1, name: "Record it, Dictate summarizes it" })).toBeInTheDocument();
  expect(screen.getByText("Get a summary, decisions and action items — with the transcript to check against.")).toBeInTheDocument();

  expect(screen.getByRole("button", { name: /Choose file/ })).toHaveTextContent("Existing video or audio: mp4, mov, m4a, mp3, wav.");
  expect(screen.getByRole("heading", { name: "Paste a link" })).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Link" })).toHaveAttribute("placeholder", "https://youtube.com/…");
  expect(screen.getByText("or Ctrl+V anywhere in the window")).toBeInTheDocument();
  expect(screen.getByText("Or drag and drop a video/audio file here")).toBeInTheDocument();

  expect(readyRows()).toEqual(["Summary API Working", "Whisper (more accurate transcripts) Working"]);
});

test("there is no recording card and no permission row: those are for recording", () => {
  render(Empty, { store });

  expect(screen.queryByText(/Record the screen|Screen Recording|Microphone/i)).not.toBeInTheDocument();
});

test("on a Mac the shortcut to paste is written with the command key", () => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
  render(Empty, { store });

  expect(screen.getByText("or ⌘V anywhere in the window")).toBeInTheDocument();
});

test("a summary API that is not set up has a button to Settings on the accounts page", async () => {
  const user = userEvent.setup();
  store.readiness = { ai: false, whisper: true, language: "en" };
  render(Empty, { store });

  expect(readyRows()).toEqual(["Summary API Open Settings", "Whisper (more accurate transcripts) Working"]);
  await user.click(screen.getByRole("button", { name: "Open Settings" }));

  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("accounts");
});

test("a Whisper model that is not there has a button to Settings on the speech page", async () => {
  const user = userEvent.setup();
  store.readiness = { ai: true, whisper: false, language: "en" };
  render(Empty, { store });

  expect(readyRows()).toEqual(["Summary API Working", "Whisper (more accurate transcripts) Download model"]);
  await user.click(screen.getByRole("button", { name: "Download model" }));

  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("speech");
});

test("while the settings are being read, nothing is said to be working or missing", () => {
  store.readiness = null;
  render(Empty, { store });

  expect(readyRows()).toEqual(["Summary API Checking…", "Whisper (more accurate transcripts) Checking…"]);
});

test("Choose file opens the dialog for files", async () => {
  const user = userEvent.setup();
  render(Empty, { store });

  await user.click(screen.getByRole("button", { name: /Choose file/ }));

  expect(api.meetingsPickFiles).toHaveBeenCalled();
});

test("a link that is typed is imported with Enter", async () => {
  const user = userEvent.setup();
  render(Empty, { store });

  await user.type(screen.getByRole("textbox", { name: "Link" }), "https://youtu.be/abc{Enter}");

  expect(api.meetingsImportLink).toHaveBeenCalledWith("https://youtu.be/abc");
});

test("a link that is refused says why, next to the field", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingsImportLink).mockRejectedValue(fakeError({ key: "meetings.error.invalid_link", detail: null }));
  render(Empty, { store });

  await user.type(screen.getByRole("textbox", { name: "Link" }), "nonsense");
  await user.click(screen.getByRole("button", { name: "Summarize" }));

  expect(await screen.findByRole("alert")).toHaveTextContent("Invalid link.");
});

test("an empty field has nothing to summarize", () => {
  render(Empty, { store });

  expect(screen.getByRole("button", { name: "Summarize" })).toBeDisabled();
});

test("speaks Vietnamese when the language is", () => {
  setLanguage("vi");
  render(Empty, { store });

  expect(screen.getByRole("heading", { level: 1, name: "Ghi lại, Dictate tóm tắt giúp bạn" })).toBeInTheDocument();
  expect(screen.getByText("Hoặc kéo thả file video/audio vào đây")).toBeInTheDocument();
  expect(screen.getByRole("region", { name: "Sẵn sàng chưa?" })).toBeInTheDocument();
});
