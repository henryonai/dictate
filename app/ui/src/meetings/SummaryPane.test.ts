import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { MeetingDetail } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeDetail, fakeSummary, stubMeetingsApi } from "../test-helpers";
import SummaryPane from "./SummaryPane.svelte";

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

function pane(overrides: Partial<MeetingDetail> = {}, canSeek = true) {
  const detail = fakeDetail({ summary: fakeSummary(), ...overrides });
  const view = render(SummaryPane, { store, detail, canSeek, onseek });
  return { ...view, detail };
}

/** The part of the pane that has this heading. */
const part = (name: string) => screen.getByRole("heading", { name }).closest("section") as HTMLElement;

test("the summary is shown in its parts, each under its heading, in the order of the summary", () => {
  pane();

  expect(screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual([
    "Summary",
    "Key points",
    "Decisions",
    "Action items",
    "Open questions",
    "Chapters",
    "Q&A",
  ]);
  expect(within(part("Summary")).getByText("We planned the quarter.")).toBeInTheDocument();
  expect(within(part("Decisions")).getByText("Ship in May")).toBeInTheDocument();
  expect(within(part("Open questions")).getByText("Who owns the docs?")).toBeInTheDocument();
});

test("the key points are rendered with their bold text and their stamps as links that seek", async () => {
  const user = userEvent.setup();
  pane();
  const points = part("Key points");

  expect(within(points).getByText("fixed").tagName).toBe("STRONG");
  await user.click(within(points).getByRole("link", { name: "[01:05]" }));
  expect(onseek).toHaveBeenCalledWith(65);
});

test("a part that the summary does not have has no heading", () => {
  pane({ summary: fakeSummary({ keyPoints: null, decisions: [], questions: [], timeline: [], actions: [] }) });

  expect(screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual(["Summary", "Q&A"]);
});

test("the sections that the window has no place for come after the others, each with its own heading", () => {
  pane({
    summary: fakeSummary({
      other: [
        { heading: "Risks", body: "- Budget **may** slip" },
        { heading: "", body: "A note before the first heading." },
      ],
    }),
  });

  expect(within(part("Risks")).getByText("may").tagName).toBe("STRONG");
  expect(screen.getByText("A note before the first heading.")).toBeInTheDocument();
  const headings = screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent);
  expect(headings.indexOf("Risks")).toBeGreaterThan(headings.indexOf("Chapters"));
});

test("action_tick_calls_set_task_and_strikes_through: a tick is written with the index of the item and shows at once", async () => {
  const user = userEvent.setup();
  pane();
  const open = screen.getByRole("checkbox", { name: "Send the budget" });
  const done = screen.getByRole("checkbox", { name: "Book the room" });

  expect(open).not.toBeChecked();
  expect(done).toBeChecked();
  expect(open.closest("label")).not.toHaveClass("done");
  expect(done.closest("label")).toHaveClass("done");

  await user.click(open);

  await vi.waitFor(() => expect(api.meetingSetTask).toHaveBeenCalledExactlyOnceWith(ID, 0, true));
  expect(open).toBeChecked();
  expect(open.closest("label")).toHaveClass("done");

  await user.click(done);
  await vi.waitFor(() => expect(api.meetingSetTask).toHaveBeenLastCalledWith(ID, 1, false));
  expect(done.closest("label")).not.toHaveClass("done");
});

test("an item shows its owner as a capsule, and an item with no owner has none", () => {
  pane();
  expect(screen.getByText("An")).toHaveClass("owner");
  expect(document.querySelectorAll(".owner")).toHaveLength(1);
});

test("a tick that could not be written is taken back, and the notice says why", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingSetTask).mockRejectedValue("Could not write tasks.json.");
  pane();
  const open = screen.getByRole("checkbox", { name: "Send the budget" });

  await user.click(open);

  await vi.waitFor(() => expect(open).not.toBeChecked());
  expect(open.closest("label")).not.toHaveClass("done");
  expect(store.notice).toBe("Could not write tasks.json.");
});

test("what the detail says wins again once it has caught up with a tick", async () => {
  const user = userEvent.setup();
  const view = pane();
  await user.click(screen.getByRole("checkbox", { name: "Send the budget" }));
  await vi.waitFor(() => expect(api.meetingSetTask).toHaveBeenCalled());

  // The library changed and the detail was read again: it has the tick.
  const ticked = fakeDetail({
    summary: fakeSummary({
      actions: [
        { index: 0, owner: "An", text: "Send the budget", done: true },
        { index: 1, owner: null, text: "Book the room", done: true },
      ],
    }),
  });
  await view.rerender({ detail: ticked });
  expect(screen.getByRole("checkbox", { name: "Send the budget" })).toBeChecked();

  // Someone unticked it elsewhere (the file was edited): the pane follows.
  await view.rerender({
    detail: fakeDetail({
      summary: fakeSummary({
        actions: [
          { index: 0, owner: "An", text: "Send the budget", done: false },
          { index: 1, owner: null, text: "Book the room", done: true },
        ],
      }),
    }),
  });
  expect(screen.getByRole("checkbox", { name: "Send the budget" })).not.toBeChecked();
});

test("copy_checklist_format: the button copies the items with their ticks, the ones just made included", async () => {
  const user = userEvent.setup();
  pane();

  await user.click(screen.getByRole("button", { name: "Copy as checklist" }));
  expect(api.copyText).toHaveBeenLastCalledWith("- [ ] **An** — Send the budget\n- [x] Book the room");
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();

  await user.click(screen.getByRole("checkbox", { name: "Send the budget" }));
  await user.click(screen.getByRole("button", { name: /Copy as checklist|Copied/ }));
  expect(api.copyText).toHaveBeenLastCalledWith("- [x] **An** — Send the budget\n- [x] Book the room");
});

test("chapters_seek_and_disable_without_player: a chapter seeks to its second, and without a player they are off", async () => {
  const user = userEvent.setup();
  const view = pane();

  await user.click(screen.getByRole("button", { name: /01:05\s+Budget/ }));
  expect(onseek).toHaveBeenCalledExactlyOnceWith(65);

  view.unmount();
  pane({}, false);
  for (const chip of within(part("Chapters")).getAllByRole("button")) expect(chip).toBeDisabled();
  await user.click(screen.getByRole("button", { name: /00:00\s+Kick-off/ }));
  expect(onseek).toHaveBeenCalledTimes(1);
});

test("a summary that is text is text: a tag or a script in any part shows as it was written", () => {
  const hostile = '<img src=x onerror="alert(1)">';
  pane({
    summary: fakeSummary({
      summary: hostile,
      keyPoints: `- ${hostile}`,
      decisions: [hostile],
      questions: [hostile],
      actions: [{ index: 0, owner: "<b>Eve</b>", text: hostile, done: false }],
      timeline: [{ seconds: 3, label: "00:03", text: hostile }],
      other: [{ heading: "<i>H</i>", body: hostile }],
    }),
  });

  expect(document.querySelector("img, b, i, script")).toBeNull();
  expect(screen.getAllByText(new RegExp(hostile.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))).length).toBeGreaterThanOrEqual(6);
});

test("no_summary_states: no API says what to add and offers the settings of the API", async () => {
  const user = userEvent.setup();
  store.readiness = { ai: false, whisper: true, language: "en" };
  pane({ summary: null });

  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  expect(
    screen.getByText("Add an API (OpenRouter, Anthropic…) so Dictate can summarize and pull out decisions and action items."),
  ).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Summarize now" })).not.toBeInTheDocument();

  await user.click(screen.getAllByRole("button", { name: "Open API settings" })[0]);
  expect(api.meetingsOpenSettings).toHaveBeenCalledWith("accounts");
});

test("no_summary_states: with an API and a transcript the summary is made on request", async () => {
  const user = userEvent.setup();
  pane({ summary: null });

  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  expect(screen.queryByText(/Add an API/)).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Summarize now" }));
  expect(api.meetingResummarize).toHaveBeenCalledExactlyOnceWith(ID);
});

test("no_summary_states: with an API and no transcript there is nothing to offer, and no questions to ask", () => {
  pane({ summary: null, transcript: [] });

  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Summarize now" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Open API settings" })).not.toBeInTheDocument();
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
});

test("while it is not known whether an API is set up, no summary is only said", () => {
  store.readiness = null;
  pane({ summary: null });

  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Summarize now" })).not.toBeInTheDocument();
  expect(screen.queryByText(/Add an API/)).not.toBeInTheDocument();
});

test("the questions are asked when there is a transcript, and the stamps of the answers seek", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingAsk).mockResolvedValue("At [01:05].");
  pane();

  await user.type(screen.getByRole("textbox", { name: "Ask about this recording" }), "When?{Enter}");
  // The key points of the fixture have a [01:05] too: the answer's is the one in the questions.
  await user.click(await within(part("Q&A")).findByRole("link", { name: "[01:05]" }));

  expect(api.meetingAsk).toHaveBeenCalledWith(ID, "When?");
  expect(onseek).toHaveBeenCalledWith(65);
});

test("the pane is Vietnamese in Vietnamese", () => {
  setLanguage("vi");
  pane();

  expect(screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual([
    "Tóm tắt",
    "Ý chính",
    "Quyết định",
    "Việc cần làm",
    "Câu hỏi còn mở",
    "Chương",
    "Hỏi đáp",
  ]);
  expect(screen.getByRole("button", { name: "Copy dạng checklist" })).toBeInTheDocument();
});
