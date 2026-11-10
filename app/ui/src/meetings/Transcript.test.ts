import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { LineView } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import Transcript from "./Transcript.svelte";

const lines: LineView[] = [
  { start: 0, stamp: "00:00", speaker: "Speaker 1", text: "Let's start." },
  { start: 65, stamp: "01:05", speaker: "Speaker 2", text: "First, the budget." },
  { start: 125, stamp: "02:05", speaker: "Speaker 1", text: "Then the hiring." },
];

const scrollIntoView = vi.fn();

beforeEach(() => {
  // jsdom has no layout, so no scrolling: the call is what the window makes of a current line.
  Element.prototype.scrollIntoView = scrollIntoView;
});

afterEach(() => {
  scrollIntoView.mockReset();
  vi.restoreAllMocks();
  setLanguage("en");
});

function show(props: Partial<Parameters<typeof render<typeof Transcript>>[1]> = {}) {
  const onseek = vi.fn();
  const onrename = vi.fn();
  const oncopy = vi.fn(async () => true);
  const view = render(Transcript, {
    lines,
    draft: false,
    currentTime: 0,
    playing: false,
    canSeek: true,
    canRename: true,
    onseek,
    onrename,
    oncopy,
    ...props,
  });
  return { ...view, onseek, onrename, oncopy };
}

/** The row of the transcript that the text is in. */
const rowOf = (text: string) => screen.getByText(text).closest("[data-line]") as HTMLElement;

test("the lines are shown with their stamp, their speaker in bold and their text, under a heading that says it follows the video", () => {
  show();

  expect(screen.getByRole("heading", { name: "Transcript" })).toBeInTheDocument();
  expect(screen.getByText("follows the video")).toBeInTheDocument();
  expect(within(rowOf("First, the budget.")).getByRole("button", { name: "01:05" })).toBeInTheDocument();
  expect(within(rowOf("First, the budget.")).getByText("Speaker 2").tagName).toMatch(/^(STRONG|BUTTON)$/);
  expect(screen.queryByText("(draft)")).not.toBeInTheDocument();
});

test("a transcript with no player does not say that it follows the video, and its stamps do not seek", () => {
  show({ canSeek: false });

  expect(screen.queryByText("follows the video")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "01:05" })).toBeDisabled();
});

test("a draft says so", () => {
  show({ draft: true });
  expect(screen.getByText("(draft)")).toBeInTheDocument();
});

test("stamp_click_seeks_player: a stamp seeks to the second of its line", async () => {
  const user = userEvent.setup();
  const { onseek } = show();

  await user.click(screen.getByRole("button", { name: "01:05" }));

  expect(onseek).toHaveBeenCalledExactlyOnceWith(65);
  expect(screen.getByRole("button", { name: "01:05" })).toHaveAttribute("title", "Jump to 01:05");
});

test("a line that has no stamp has no button to seek with", () => {
  show({ lines: [{ start: 0, stamp: "", speaker: null, text: "Loose words." }] });
  expect(screen.getByText("Loose words.")).toBeInTheDocument();
  expect(within(rowOf("Loose words.")).queryByRole("button")).not.toBeInTheDocument();
});

test("transcript_highlights_current_line_while_playing: the last line that began (0.3 s early) is the current one, and it is scrolled to the centre", () => {
  show({ currentTime: 70, playing: true });

  expect(rowOf("First, the budget.")).toHaveAttribute("aria-current", "true");
  expect(rowOf("Let's start.")).not.toHaveAttribute("aria-current");
  expect(rowOf("Then the hiring.")).not.toHaveAttribute("aria-current");
  expect(scrollIntoView).toHaveBeenCalledWith(expect.objectContaining({ block: "center" }));
});

test("a line is current 0.3 s before it begins", () => {
  // The second line begins at 65 s.
  const before = show({ currentTime: 64.6, playing: true });
  expect(rowOf("Let's start.")).toHaveAttribute("aria-current", "true");
  before.unmount();

  show({ currentTime: 64.8, playing: true });
  expect(rowOf("First, the budget.")).toHaveAttribute("aria-current", "true");
});

test("nothing is highlighted or scrolled to while the player is paused", () => {
  show({ currentTime: 70, playing: false });

  expect(document.querySelector("[aria-current]")).toBeNull();
  expect(scrollIntoView).not.toHaveBeenCalled();
});

test("before the first line there is no current line", () => {
  show({ lines: lines.slice(1), currentTime: 10, playing: true });
  expect(document.querySelector("[aria-current]")).toBeNull();
});

test("it scrolls when the current line changes and not on each tick that leaves it", async () => {
  const view = show({ currentTime: 70, playing: true });
  expect(scrollIntoView).toHaveBeenCalledTimes(1);

  await view.rerender({ currentTime: 80 });
  expect(scrollIntoView).toHaveBeenCalledTimes(1);

  await view.rerender({ currentTime: 130 });
  expect(rowOf("Then the hiring.")).toHaveAttribute("aria-current", "true");
  expect(scrollIntoView).toHaveBeenCalledTimes(2);
});

test("no transcript says so", () => {
  show({ lines: [] });
  expect(screen.getByText("No transcript yet.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
});

test("the name of a speaker asks to rename that speaker, and the heading's button asks to rename all", async () => {
  const user = userEvent.setup();
  const { onrename } = show();

  await user.click(within(rowOf("First, the budget.")).getByRole("button", { name: "Speaker 2" }));
  expect(onrename).toHaveBeenLastCalledWith("Speaker 2");
  expect(within(rowOf("First, the budget.")).getByRole("button", { name: "Speaker 2" })).toHaveAttribute("title", "Rename Speaker 2");

  await user.click(screen.getByRole("button", { name: "Rename speakers" }));
  expect(onrename).toHaveBeenLastCalledWith();
});

test("a transcript that can't be renamed (live captions, or a job that runs) has plain names and no button", () => {
  show({ canRename: false });

  expect(screen.queryByRole("button", { name: "Rename speakers" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Speaker 2" })).not.toBeInTheDocument();
  expect(screen.getByText("Speaker 2")).toBeInTheDocument();
});

test("Copy copies, and says that it did", async () => {
  const user = userEvent.setup();
  const { oncopy } = show();

  await user.click(screen.getByRole("button", { name: "Copy" }));

  expect(oncopy).toHaveBeenCalledOnce();
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
});

test("Copy that failed does not say that it worked", async () => {
  const user = userEvent.setup();
  show({ oncopy: vi.fn(async () => false) });

  await user.click(screen.getByRole("button", { name: "Copy" }));

  expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copied" })).not.toBeInTheDocument();
});

test("what a recording says is text: a tag in a speaker or a line shows as it was written", () => {
  const hostile = '<img src=x onerror="alert(1)">';
  show({ lines: [{ start: 0, stamp: "00:00", speaker: "<b>Eve</b>", text: hostile }] });

  expect(screen.getByText(hostile)).toBeInTheDocument();
  expect(screen.getByText("<b>Eve</b>")).toBeInTheDocument();
  expect(document.querySelector("img, b")).toBeNull();
});

test("the headings and the words are Vietnamese in Vietnamese", () => {
  setLanguage("vi");
  show({ draft: true });

  expect(screen.getByText("tự cuộn theo video")).toBeInTheDocument();
  expect(screen.getByText("(bản nháp)")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Đổi tên người nói" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "01:05" })).toHaveAttribute("title", "Tua tới 01:05");
});
