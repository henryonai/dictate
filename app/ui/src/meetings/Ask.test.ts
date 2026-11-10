import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeError, stubMeetingsApi } from "../test-helpers";
import Ask from "./Ask.svelte";

const ID = "2026-03-02 10-00-00";
let store: MeetingsStore;
const onseek = vi.fn();

beforeEach(() => {
  stubMeetingsApi();
  store = new MeetingsStore();
  store.readiness = { ai: true, whisper: true, language: "en" };
});

afterEach(() => {
  onseek.mockReset();
  vi.restoreAllMocks();
  setLanguage("en");
});

const field = () => screen.getByRole("textbox", { name: "Ask about this recording" });

function ask() {
  return render(Ask, { store, id: ID, onseek });
}

test("the field has the words of the example, under the heading of the questions", () => {
  ask();
  expect(screen.getByRole("heading", { name: "Q&A" })).toBeInTheDocument();
  expect(field()).toHaveAttribute("placeholder", "Ask about this recording… (e.g. who owns the docs?)");
});

test("ask_submits_once_and_renders_answer: Enter asks, the field is cleared, and the answer is shown with its stamps as links", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingAsk).mockResolvedValue("**An** owns the docs, see [01:05].");
  ask();

  await user.type(field(), "Who owns the docs?{Enter}");

  expect(api.meetingAsk).toHaveBeenCalledExactlyOnceWith(ID, "Who owns the docs?");
  expect(field()).toHaveValue("");
  const answer = await screen.findByText(/owns the docs, see/);
  expect(within(answer).getByText("An").tagName).toBe("STRONG");
  expect(screen.getByText("Who owns the docs?")).toBeInTheDocument();

  await user.click(screen.getByRole("link", { name: "[01:05]" }));
  expect(onseek).toHaveBeenCalledWith(65);
});

test("the field waits for the answer: it is disabled, with a spinner, and takes the focus back when the answer comes", async () => {
  const user = userEvent.setup();
  let answer: (text: string) => void = () => {};
  vi.mocked(api.meetingAsk).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
  ask();

  await user.type(field(), "Who?{Enter}");
  expect(field()).toBeDisabled();
  expect(screen.getByRole("status", { name: "Working on the answer…" })).toBeInTheDocument();

  // A second question is not sent while the first has no answer.
  await user.keyboard("{Enter}");
  expect(api.meetingAsk).toHaveBeenCalledTimes(1);

  answer("Nobody.");
  await screen.findByText("Nobody.");
  expect(field()).toBeEnabled();
  await vi.waitFor(() => expect(field()).toHaveFocus());
  expect(screen.queryByRole("status")).not.toBeInTheDocument();
});

test("an empty question is not asked", async () => {
  const user = userEvent.setup();
  ask();

  await user.type(field(), "   {Enter}");

  expect(api.meetingAsk).not.toHaveBeenCalled();
});

test("ask_error_shows_message: a refusal is shown as Error: and its message", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingAsk).mockRejectedValue(fakeError({ key: "meetings.error.no_transcript", detail: null }));
  ask();

  await user.type(field(), "Who?{Enter}");

  expect(await screen.findByText("Error: No transcript yet.")).toBeInTheDocument();
  // The question stays above it, and the field is there for another.
  expect(screen.getByText("Who?")).toBeInTheDocument();
  expect(field()).toBeEnabled();
});

test("a refusal because the API is not set up offers the API settings", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingAsk).mockRejectedValue(fakeError({ kind: "Api", key: "meetings.error.no_ai", detail: null }));
  ask();

  await user.type(field(), "Who?{Enter}");
  expect(await screen.findByText("Error: API not set up in Settings.")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Open API settings" }));
  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("accounts");
});

test("an error of another kind has no button", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingAsk).mockRejectedValue("The IPC failed.");
  ask();

  await user.type(field(), "Who?{Enter}");

  expect(await screen.findByText("Error: The IPC failed.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Open API settings" })).not.toBeInTheDocument();
});

test("the questions of the session are shown again when the recording is shown again", () => {
  store.asks.set(ID, [
    { question: "First?", answer: "One, at [00:10].", error: null },
    { question: "Second?", answer: null, error: fakeError({ detail: "Boom" }) },
  ]);
  store.asks.set("another", [{ question: "Not mine?", answer: "No.", error: null }]);

  ask();

  expect(screen.getByText("First?")).toBeInTheDocument();
  expect(screen.getByText("Second?")).toBeInTheDocument();
  expect(screen.getByText("Error: Boom")).toBeInTheDocument();
  expect(screen.queryByText("Not mine?")).not.toBeInTheDocument();
});

test("without an API the field is off, and the way to set one up is offered", async () => {
  const user = userEvent.setup();
  store.readiness = { ai: false, whisper: true, language: "en" };
  ask();

  expect(field()).toBeDisabled();
  expect(screen.getByText("API not set up in Settings.")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Open API settings" }));
  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("accounts");
});

test("while it is not known whether an API is set up the field is on: the Rust side refuses a question that has no API", () => {
  store.readiness = null;
  ask();
  expect(field()).toBeEnabled();
});

test("what a model answers is text: a tag in an answer or in a question shows as it was written", async () => {
  const user = userEvent.setup();
  const hostile = '<img src=x onerror="alert(1)"> [x](javascript:alert(1))';
  vi.mocked(api.meetingAsk).mockResolvedValue(hostile);
  ask();

  await user.type(field(), "<b>q</b>{Enter}");

  expect(await screen.findByText(hostile)).toBeInTheDocument();
  expect(screen.getByText("<b>q</b>")).toBeInTheDocument();
  expect(document.querySelector("img, b, a")).toBeNull();
});

test("the words are Vietnamese in Vietnamese", () => {
  setLanguage("vi");
  store.asks.set(ID, [{ question: "Ai?", answer: null, error: fakeError({ detail: "Hỏng" }) }]);

  ask();

  expect(screen.getByRole("heading", { name: "Hỏi đáp" })).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Hỏi về bản ghi này" })).toHaveAttribute(
    "placeholder",
    "Hỏi về bản ghi này… (VD: ai phụ trách phần tài liệu?)",
  );
  expect(screen.getByText("Lỗi: Hỏng")).toBeInTheDocument();
});
