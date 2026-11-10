import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { setLanguage } from "../lib/i18n";
import { fakeError, fakeJob, fakeRow } from "../test-helpers";
import Row from "./Row.svelte";

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

const subtitle = (container: HTMLElement) => container.querySelector(".subtitle");

test("row_subtitles_for_done_failed_busy: a recording that is done says when, how long, and how many action items are open", () => {
  const { container } = render(Row, { row: fakeRow({ duration: 720, openTasks: 2 }), selected: false, onselect: vi.fn() });

  // The time is the system's own way of writing it ("10:00 AM", "10:00").
  expect(subtitle(container)?.textContent).toMatch(/^10:00\s?(AM)? · 12 min · 2 action items$/);
  expect(screen.getByText("Weekly planning")).toBeInTheDocument();
});

test("a single open action item, none, or no duration known is told the shorter way", () => {
  const one = render(Row, { row: fakeRow({ openTasks: 1 }), selected: false, onselect: vi.fn() });
  expect(subtitle(one.container)?.textContent).toMatch(/ · 12 min · 1 action item$/);
  one.unmount();

  const none = render(Row, { row: fakeRow({ openTasks: 0, duration: null }), selected: false, onselect: vi.fn() });
  expect(subtitle(none.container)?.textContent).toMatch(/^10:00\s?(AM)?$/);
});

test("a recording with no summary says that it has none", () => {
  const { container } = render(Row, { row: fakeRow({ hasSummary: false, openTasks: 0 }), selected: false, onselect: vi.fn() });

  expect(subtitle(container)).toHaveTextContent("not summarized");
});

test("a recording with no date shows no time", () => {
  const { container } = render(Row, { row: fakeRow({ created: 0, duration: 90, openTasks: 0 }), selected: false, onselect: vi.fn() });

  expect(subtitle(container)?.textContent).toBe("2 min");
});

test("a recording that failed says to click for how to fix it, in red, and a cancelled one says so", () => {
  const failed = render(Row, {
    row: fakeRow({ error: fakeError({ kind: "Api", key: "meetings.error.summary_failed" }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(subtitle(failed.container)).toHaveTextContent("Error · click for how to fix");
  expect(subtitle(failed.container)).toHaveClass("failed");
  failed.unmount();

  const cancelled = render(Row, {
    row: fakeRow({ error: fakeError({ kind: "Cancelled", key: "meetings.error.cancelled", detail: null }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(subtitle(cancelled.container)).toHaveTextContent(/^Cancelled$/);
});

test("a recording that is being processed shows the step and the percent, and a figure that is not known shows no percent", () => {
  const busy = render(Row, {
    row: fakeRow({ live: fakeJob({ step: "transcript", progress: 0.424 }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(subtitle(busy.container)).toHaveTextContent("Transcribing · 42%");
  expect(subtitle(busy.container)).toHaveClass("busy");
  expect(busy.container.querySelector(".spinner")).not.toBeNull();
  busy.unmount();

  const unknown = render(Row, { row: fakeRow({ live: fakeJob({ step: "audio", progress: null }) }), selected: false, onselect: vi.fn() });
  expect(subtitle(unknown.container)).toHaveTextContent(/^Extract audio$/);
  unknown.unmount();

  const downloading = render(Row, { row: fakeRow({ live: fakeJob({ step: "download", progress: 1 }) }), selected: false, onselect: vi.fn() });
  expect(subtitle(downloading.container)).toHaveTextContent("Downloading · 100%");
});

test("a job that has no step yet is waiting, not broken", () => {
  const { container } = render(Row, { row: fakeRow({ live: fakeJob({ step: null }) }), selected: false, onselect: vi.fn() });

  expect(subtitle(container)).toHaveTextContent(/^Waiting$/);
});

test("a job that is running wins over the error of an earlier run", () => {
  const { container } = render(Row, {
    row: fakeRow({ live: fakeJob({ step: "summary" }), error: fakeError() }),
    selected: false,
    onselect: vi.fn(),
  });

  expect(subtitle(container)).toHaveTextContent("Summarizing");
});

test("the icon tells where the recording came from, and a red one that it failed", () => {
  const icon = (row: Parameters<typeof fakeRow>[0]) => {
    const view = render(Row, { row: fakeRow(row), selected: false, onselect: vi.fn() });
    const found = view.container.querySelector("[data-icon]")?.getAttribute("data-icon");
    view.unmount();
    return found;
  };

  expect(icon({ kind: "screen" })).toBe("screen");
  expect(icon({ kind: "link" })).toBe("link");
  expect(icon({ kind: "file" })).toBe("file");
  expect(icon({ kind: "link", error: fakeError() })).toBe("error");
});

test("the row is a button that selects its recording, and says when it is the selected one", async () => {
  const user = userEvent.setup();
  const onselect = vi.fn();
  const { rerender } = render(Row, { row: fakeRow({ id: "m1" }), selected: false, onselect });
  const button = screen.getByRole("button");
  expect(button).not.toHaveAttribute("aria-current");

  await user.click(button);
  expect(onselect).toHaveBeenCalledWith("m1");

  rerender({ row: fakeRow({ id: "m1" }), selected: true, onselect });
  expect(screen.getByRole("button")).toHaveAttribute("aria-current", "true");
});

test("a title that looks like HTML is shown as text", () => {
  const { container } = render(Row, {
    row: fakeRow({ title: '<img src=x onerror="alert(1)"> Standup' }),
    selected: false,
    onselect: vi.fn(),
  });

  expect(container.querySelector("img")).toBeNull();
  expect(screen.getByText('<img src=x onerror="alert(1)"> Standup')).toBeInTheDocument();
});

test("speaks Vietnamese when the language is", () => {
  setLanguage("vi");
  const { container } = render(Row, { row: fakeRow({ hasSummary: false }), selected: false, onselect: vi.fn() });

  expect(subtitle(container)).toHaveTextContent("chưa tóm tắt");
});
