import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { declarationsOf, parseCss, styleOf } from "../css-rules";
import { setLanguage } from "../lib/i18n";
import { fakeError, fakeJob, fakeRow } from "../test-helpers";
import Row from "./Row.svelte";

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

const subtitle = (container: HTMLElement) => container.querySelector(".subtitle");
// jsdom does not load the styles of a component: what a test can say about the look is what the source says.
const styles = parseCss(styleOf(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "Row.svelte"), "utf8")));

const badges = (container: HTMLElement) => [...container.querySelectorAll(".badge")];
const badge = (container: HTMLElement) => container.querySelector(".badge");

const LONG = "Quarterly planning with the whole platform team, the design guild and the data group, part two of three";

test("the_title_is_one_line_with_the_full_title_on_hover", () => {
  const { container } = render(Row, { row: fakeRow({ title: LONG }), selected: false, onselect: vi.fn() });

  // The whole title is in the page, so that nothing is cut but the way it is drawn: a tooltip and the name of the button say all of it.
  const title = screen.getByText(LONG);
  expect(title).toHaveAttribute("title", LONG);
  expect(screen.getByRole("button").getAttribute("aria-label")?.startsWith(LONG)).toBe(true);
  expect(container.querySelectorAll(".title")).toHaveLength(1);

  // One line with an ellipsis: jsdom has no layout, so the rule is read from the style of the component.
  const rule = declarationsOf(styles, ".title");
  expect(rule["white-space"]).toBe("nowrap");
  expect(rule.overflow).toBe("hidden");
  expect(rule["text-overflow"]).toBe("ellipsis");
  expect(rule["-webkit-line-clamp"]).toBeUndefined();
});

test("a_failed_row_shows_the_failed_badge_and_opens_the_error_card", async () => {
  const user = userEvent.setup();
  const onselect = vi.fn();
  const { container } = render(Row, {
    row: fakeRow({ id: "m7", error: fakeError({ kind: "Api", key: "meetings.error.summary_failed" }), openTasks: 3 }),
    selected: false,
    onselect,
  });

  // One badge, red, and the tooltip says what a click does. The tone is the danger token (Row.svelte's style, below).
  expect(badges(container)).toHaveLength(1);
  expect(badge(container)).toHaveTextContent(/^failed$/);
  expect(badge(container)).toHaveClass("failed");
  expect(badge(container)).toHaveAttribute("title", "Click for how to fix");
  expect(declarationsOf(styles, ".badge.failed").color).toBe("var(--danger)");
  expect(subtitle(container)).toHaveTextContent("12 min");
  expect(container.querySelector("[data-icon]")).toHaveAttribute("data-icon", "error");

  // The click is the one of every row: the window shows the error card of the recording that was picked.
  await user.click(screen.getByRole("button"));
  expect(onselect).toHaveBeenCalledWith("m7");
});

test("a_summarized_row_shows_the_action_count", () => {
  const many = render(Row, { row: fakeRow({ duration: 720, openTasks: 2 }), selected: false, onselect: vi.fn() });
  // Duration first, then the one badge.
  expect([...subtitle(many.container)!.children].map((child) => child.textContent?.trim())).toEqual(["12 min", "2 actions"]);
  expect(badges(many.container)).toHaveLength(1);
  expect(badge(many.container)).not.toHaveClass("failed");
  many.unmount();

  const one = render(Row, { row: fakeRow({ openTasks: 1 }), selected: false, onselect: vi.fn() });
  expect(badge(one.container)).toHaveTextContent(/^1 action$/);
  one.unmount();

  // No open action, no badge: the line is the duration alone. (The count is of the open ones, as the Actions tab counts.)
  const none = render(Row, { row: fakeRow({ openTasks: 0 }), selected: false, onselect: vi.fn() });
  expect(badges(none.container)).toHaveLength(0);
  expect(subtitle(none.container)?.textContent?.trim()).toBe("12 min");
});

test("a_live_row_shows_processing_with_percent", () => {
  const busy = render(Row, {
    row: fakeRow({ live: fakeJob({ step: "transcript", progress: 0.424 }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(badges(busy.container)).toHaveLength(1);
  expect(badge(busy.container)).toHaveTextContent(/^processing 42%$/);
  expect(badge(busy.container)).toHaveClass("busy");
  // The step is not lost: it is the tooltip.
  expect(badge(busy.container)).toHaveAttribute("title", "Transcribing");
  expect(busy.container.querySelector(".spinner")).not.toBeNull();
  busy.unmount();

  // A figure that is not known, or one that is out of range, is not shown as a number that is wrong.
  const unknown = render(Row, { row: fakeRow({ live: fakeJob({ step: null, progress: null }) }), selected: false, onselect: vi.fn() });
  expect(badge(unknown.container)).toHaveTextContent(/^processing$/);
  expect(badge(unknown.container)).toHaveAttribute("title", "Waiting");
  unknown.unmount();

  const done = render(Row, { row: fakeRow({ live: fakeJob({ step: "download", progress: 7 }) }), selected: false, onselect: vi.fn() });
  expect(badge(done.container)).toHaveTextContent(/^processing 100%$/);
});

test("a recording with no summary says that it has none, and that wins over the count of its action items", () => {
  const { container } = render(Row, { row: fakeRow({ hasSummary: false, openTasks: 4 }), selected: false, onselect: vi.fn() });

  expect(badges(container)).toHaveLength(1);
  expect(badge(container)).toHaveTextContent(/^not summarized$/);
});

test("a recording that failed or is being processed before it has a summary says so, and the missing summary does not hide it", () => {
  // The shapes that really come: a recording is "not summarized" while its job runs and after its job failed. The order of the checks
  // decides which badge a row has: failed and processing are asked before the summary.
  const failed = render(Row, {
    row: fakeRow({ hasSummary: false, openTasks: 0, error: fakeError({ kind: "Api", key: "meetings.error.summary_failed" }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(badges(failed.container)).toHaveLength(1);
  expect(badge(failed.container)).toHaveTextContent(/^failed$/);
  expect(badge(failed.container)).toHaveClass("failed");
  failed.unmount();

  const cancelled = render(Row, {
    row: fakeRow({ hasSummary: false, openTasks: 0, error: fakeError({ kind: "Cancelled", key: "meetings.error.cancelled", detail: null }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(badge(cancelled.container)).toHaveTextContent(/^Cancelled$/);
  cancelled.unmount();

  const busy = render(Row, {
    row: fakeRow({ hasSummary: false, openTasks: 0, duration: null, live: fakeJob({ step: "summary", progress: 0.67 }) }),
    selected: false,
    onselect: vi.fn(),
  });
  expect(badges(busy.container)).toHaveLength(1);
  expect(badge(busy.container)).toHaveTextContent(/^processing 67%$/);
  expect(badge(busy.container)).toHaveClass("busy");
});

test("a cancelled recording says so in the place of the failed badge, and keeps the red", () => {
  const { container } = render(Row, {
    row: fakeRow({ error: fakeError({ kind: "Cancelled", key: "meetings.error.cancelled", detail: null }) }),
    selected: false,
    onselect: vi.fn(),
  });

  expect(badges(container)).toHaveLength(1);
  expect(badge(container)).toHaveTextContent(/^Cancelled$/);
  expect(badge(container)).toHaveClass("failed");
});

test("a job that is running wins over the error of an earlier run", () => {
  const { container } = render(Row, {
    row: fakeRow({ live: fakeJob({ step: "summary", progress: 0.5 }), error: fakeError() }),
    selected: false,
    onselect: vi.fn(),
  });

  expect(badges(container)).toHaveLength(1);
  expect(badge(container)).toHaveTextContent("processing 50%");
});

test("a recording with no duration known shows the badge alone, and the time of day is no longer on the line", () => {
  const { container } = render(Row, { row: fakeRow({ duration: null, openTasks: 2 }), selected: false, onselect: vi.fn() });

  expect(subtitle(container)?.textContent?.trim()).toBe("2 actions");
});

test("the name of the button says the title, how long it is and its state, to a reader of the screen", () => {
  const ok = render(Row, { row: fakeRow({ title: LONG, duration: 720, openTasks: 2 }), selected: false, onselect: vi.fn() });
  expect(screen.getByRole("button")).toHaveAccessibleName(`${LONG}, 12 min, 2 actions`);
  ok.unmount();

  render(Row, { row: fakeRow({ error: fakeError() }), selected: false, onselect: vi.fn() });
  expect(screen.getByRole("button")).toHaveAccessibleName("Weekly planning, 12 min, failed, Click for how to fix");
});

test("the name of the button is in Vietnamese when the language is", () => {
  setLanguage("vi");
  const ok = render(Row, { row: fakeRow({ title: LONG, duration: 720, openTasks: 2 }), selected: false, onselect: vi.fn() });
  expect(screen.getByRole("button")).toHaveAccessibleName(`${LONG}, 12 phút, 2 việc`);
  ok.unmount();

  render(Row, { row: fakeRow({ error: fakeError() }), selected: false, onselect: vi.fn() });
  expect(screen.getByRole("button")).toHaveAccessibleName("Weekly planning, 12 phút, lỗi, Bấm để xem cách sửa");
});

test("nothing of a row is wider than the list: every part of it may shrink and the text that does not fit ends in an ellipsis", () => {
  // The list is 240 px at the narrowest (`min-width` of the aside), and a row loses 56 px of it to its padding, border, icon and gap.
  const text = declarationsOf(styles, ".text");
  expect(text["min-width"]).toBe("0");
  const line = declarationsOf(styles, ".subtitle");
  expect(line["min-width"]).toBe("0");
  expect(line["white-space"]).toBe("nowrap");
  // What is too wide is the duration, which gives way first and ends in an ellipsis ("1 giờ 5 p…")...
  const duration = declarationsOf(styles, ".duration");
  expect(duration.flex).toBe("0 1 auto");
  expect(duration["min-width"]).toBe("0");
  expect(duration.overflow).toBe("hidden");
  expect(duration["text-overflow"]).toBe("ellipsis");
  // ...so that the badge keeps its figure ("đang xử lý 100%", "12 actions"): it does not shrink while the duration has room to give. Its words
  // are cut with an ellipsis only when the badge alone is wider than the line.
  const badgeRule = declarationsOf(styles, ".badge");
  expect(badgeRule.flex).toBe("none");
  expect(badgeRule["min-width"]).toBe("0");
  expect(badgeRule["max-width"]).toBe("100%");
  const label = declarationsOf(styles, ".badge > span:last-child");
  expect(label.overflow).toBe("hidden");
  expect(label["text-overflow"]).toBe("ellipsis");
  expect(label["min-width"]).toBe("0");
});

test("the badges are drawn in the tokens of the theme: the danger red for a failure, the warning for work in progress", () => {
  expect(declarationsOf(styles, ".badge.failed")["border-color"]).toBe("var(--danger)");
  expect(declarationsOf(styles, ".badge.busy").color).toBe("var(--warn-fg)");
  expect(declarationsOf(styles, ".badge.busy")["border-color"]).toBe("var(--warn)");
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
  const states: [Parameters<typeof fakeRow>[0], string][] = [
    [{ hasSummary: false }, "chưa tóm tắt"],
    [{ error: fakeError() }, "lỗi"],
    [{ live: fakeJob({ step: "transcript", progress: 0.4 }) }, "đang xử lý 40%"],
    [{ openTasks: 2 }, "2 việc"],
    [{ openTasks: 1 }, "1 việc"],
  ];
  for (const [row, text] of states) {
    const view = render(Row, { row: fakeRow(row), selected: false, onselect: vi.fn() });
    expect(badge(view.container), text).toHaveTextContent(new RegExp(`^${text}$`));
    view.unmount();
  }

  render(Row, { row: fakeRow({ error: fakeError() }), selected: false, onselect: vi.fn() });
  expect(badge(document.body)).toHaveAttribute("title", "Bấm để xem cách sửa");
});
