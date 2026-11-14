import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { cleanup, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { declarationsOf, parseCss, styleOf } from "../css-rules";
import { api } from "../lib/api";
import type { MeetingDetail } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeDetail, fakeRow, fakeSummary, stubMeetingsApi } from "../test-helpers";
import SummaryPane from "./SummaryPane.svelte";

const ID = "2026-03-02 10-00-00";
/** Where the last tab is kept. */
const TAB_KEY = "loah.detail.tab";
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

/**
 * The pane of a recording. It opens on the tab that was chosen last time, so a test about what is in a tab starts on it by saying which
 * one (the choice is kept like that; the tests of the tabs themselves click).
 */
function pane(overrides: Partial<MeetingDetail> = {}, canSeek = true, tab: "summary" | "actions" | "chapters" | "ask" = "summary") {
  localStorage.setItem(TAB_KEY, tab);
  const detail = fakeDetail({ summary: fakeSummary(), ...overrides });
  const view = render(SummaryPane, { store, detail, canSeek, onseek });
  return { ...view, detail };
}

/** The part of the pane that has this heading. */
const part = (name: string) => screen.getByRole("heading", { name }).closest("section") as HTMLElement;
/** A tab by its name, whatever count it has after it ("Actions" is "Actions (2)"). */
const tab = (name: string) => screen.getByRole("tab", { name: new RegExp(`^${name}( \\(\\d+\\))?$`) });
const headings = () => screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent);

test("the Summary tab has the overview, the key points, the decisions and the open questions, each under its heading, in that order", () => {
  pane();

  expect(headings()).toEqual(["Summary", "Key points", "Decisions", "Open questions"]);
  expect(within(part("Summary")).getByText("We planned the quarter.")).toBeInTheDocument();
  expect(within(part("Decisions")).getByText("Ship in May")).toBeInTheDocument();
  expect(within(part("Open questions")).getByText("Who owns the docs?")).toBeInTheDocument();
});

test("the other tabs have the rest: the action items, the chapters, and the questions that are asked", () => {
  pane({}, true, "actions");
  expect(headings()).toEqual(["Action items"]);
  expect(screen.getByRole("checkbox", { name: "Send the budget" })).toBeInTheDocument();
  cleanup();

  pane({}, true, "chapters");
  expect(headings()).toEqual(["Chapters"]);
  expect(screen.getByRole("button", { name: /01:05\s+Budget/ })).toBeInTheDocument();
  cleanup();

  pane({}, true, "ask");
  expect(headings()).toEqual(["Q&A"]);
  expect(screen.getByRole("textbox", { name: "Ask about this recording" })).toBeInTheDocument();
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

  expect(headings()).toEqual(["Summary"]);
});

test("a tab that has nothing says so", () => {
  const empty = fakeSummary({ timeline: [], actions: [] });
  pane({ summary: empty }, true, "actions");
  expect(screen.getByText("No action items in this recording.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Copy as checklist" })).not.toBeInTheDocument();
  cleanup();

  pane({ summary: empty }, true, "chapters");
  expect(screen.getByText("No chapters in this recording.")).toBeInTheDocument();
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
  expect(headings().indexOf("Risks")).toBeGreaterThan(headings().indexOf("Open questions"));
  // They are of the Summary tab, and the other tabs do not have them.
  expect(screen.getByRole("tabpanel")).toContainElement(part("Risks"));
});

test("action_tick_calls_set_task_and_strikes_through: a tick is written with the index of the item and shows at once", async () => {
  const user = userEvent.setup();
  pane({}, true, "actions");
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

test("the text of an action item is rendered as the other texts are: bold, and stamps that seek without ticking the item", async () => {
  const user = userEvent.setup();
  pane({ summary: fakeSummary({ actions: [{ index: 0, owner: "An", text: "Send **the budget** at [01:05]", done: false }] }) }, true, "actions");
  const item = screen.getByRole("checkbox", { name: "Send the budget at [01:05]" });

  expect(within(item.closest("label") as HTMLElement).getByText("the budget").tagName).toBe("STRONG");
  await user.click(within(item.closest("label") as HTMLElement).getByRole("link", { name: "[01:05]" }));

  expect(onseek).toHaveBeenCalledExactlyOnceWith(65);
  expect(item).not.toBeChecked();
  expect(api.meetingSetTask).not.toHaveBeenCalled();
});

test("an item with a stamp and no player has the stamp as plain text", () => {
  pane({ summary: fakeSummary({ keyPoints: null, actions: [{ index: 0, owner: null, text: "Look at [01:05]", done: false }] }) }, false, "actions");
  expect(screen.getByText("[01:05]")).toHaveAttribute("data-inert");
});

test("ask_lives_in_its_tab: the field is in the Ask tab and not under the summary, and the tab has it whole", async () => {
  const user = userEvent.setup();
  pane();

  // Not pinned under the parts of the summary any more: the Summary tab has no question and no field.
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "Q&A" })).not.toBeInTheDocument();

  await user.click(tab("Ask"));

  const panel = screen.getByRole("tabpanel");
  expect(tab("Ask")).toHaveAttribute("aria-selected", "true");
  expect(within(panel).getByRole("textbox", { name: "Ask about this recording" })).toBeInTheDocument();
  expect(within(panel).getByRole("heading", { name: "Q&A" })).toBeInTheDocument();
  // The parts of the summary are not in it.
  expect(screen.queryByRole("heading", { name: "Key points" })).not.toBeInTheDocument();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
});

test("a recording with no transcript has nothing to ask: the Ask tab is not offered", () => {
  pane({ transcript: [] });

  expect(screen.queryByRole("tab", { name: "Ask" })).not.toBeInTheDocument();
  expect(screen.getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual(["Summary", "Actions (2)", "Chapters (2)"]);
});

test("without a summary the tabs are the Summary tab, which says so, and Ask when there is a transcript", () => {
  pane({ summary: null });

  expect(screen.getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual(["Summary", "Ask"]);
  expect(screen.getByText("No summary yet").closest(".sections")).not.toBeNull();
});

test("an item shows its owner as a capsule, and an item with no owner has none", () => {
  pane({}, true, "actions");
  expect(screen.getByText("An")).toHaveClass("owner");
  expect(document.querySelectorAll(".owner")).toHaveLength(1);
});

test("a tick that could not be written is taken back, and the notice says why", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingSetTask).mockRejectedValue("Could not write tasks.json.");
  pane({}, true, "actions");
  const open = screen.getByRole("checkbox", { name: "Send the budget" });

  await user.click(open);

  await vi.waitFor(() => expect(open).not.toBeChecked());
  expect(open.closest("label")).not.toHaveClass("done");
  expect(store.notice).toBe("Could not write tasks.json.");
});

test("what the detail says wins again once it has caught up with a tick", async () => {
  const user = userEvent.setup();
  const view = pane({}, true, "actions");
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
  pane({}, true, "actions");

  await user.click(screen.getByRole("button", { name: "Copy as checklist" }));
  expect(api.copyText).toHaveBeenLastCalledWith("- [ ] **An** — Send the budget\n- [x] Book the room");
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();

  await user.click(screen.getByRole("checkbox", { name: "Send the budget" }));
  await user.click(screen.getByRole("button", { name: /Copy as checklist|Copied/ }));
  expect(api.copyText).toHaveBeenLastCalledWith("- [x] **An** — Send the budget\n- [x] Book the room");
});

test("chapters_seek_and_disable_without_player: a chapter seeks to its second, and without a player they are off", async () => {
  const user = userEvent.setup();
  const view = pane({}, true, "chapters");

  await user.click(screen.getByRole("button", { name: /01:05\s+Budget/ }));
  expect(onseek).toHaveBeenCalledExactlyOnceWith(65);

  view.unmount();
  pane({}, false, "chapters");
  for (const chip of within(part("Chapters")).getAllByRole("button")) expect(chip).toBeDisabled();
  await user.click(screen.getByRole("button", { name: /00:00\s+Kick-off/ }));
  expect(onseek).toHaveBeenCalledTimes(1);
});

test("the chapters are a vertical list of chips, one under the other", () => {
  pane({}, true, "chapters");

  const list = within(part("Chapters")).getByRole("list");
  expect(within(list).getAllByRole("listitem")).toHaveLength(2);
  expect(declarationsOf(styles, ".chapters")["flex-direction"]).toBe("column");
});

test("a summary that is text is text: a tag or a script in any part shows as it was written", async () => {
  const user = userEvent.setup();
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

  const shown = () => screen.getAllByText(new RegExp(hostile.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))).length;

  // The overview, the key points, the decisions, the questions and the other section; then the action, then the chapter.
  expect(document.querySelector("img, b, i, script")).toBeNull();
  expect(shown()).toBe(5);
  await user.click(tab("Actions"));
  expect(document.querySelector("img, b, i, script")).toBeNull();
  expect(shown()).toBe(1);
  expect(screen.getByText("<b>Eve</b>")).toHaveClass("owner");
  await user.click(tab("Chapters"));
  expect(document.querySelector("img, b, i, script")).toBeNull();
  expect(shown()).toBe(1);
});

test("no_summary_states: no API says what to add and offers the settings of the API", async () => {
  const user = userEvent.setup();
  store.readiness = { ai: false, whisper: true, language: "en" };
  pane({ summary: null });

  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  expect(
    screen.getByText("Add an API (OpenRouter, Anthropic…) so Loah can summarize and pull out decisions and action items."),
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
  pane({}, true, "ask");

  await user.type(screen.getByRole("textbox", { name: "Ask about this recording" }), "When?{Enter}");
  await user.click(await within(part("Q&A")).findByRole("link", { name: "[01:05]" }));

  expect(api.meetingAsk).toHaveBeenCalledWith(ID, "When?");
  expect(onseek).toHaveBeenCalledWith(65);
});

test("the pane is Vietnamese in Vietnamese", async () => {
  const user = userEvent.setup();
  setLanguage("vi");
  pane();

  expect(screen.getByRole("tablist", { name: "Các phần của bản ghi" })).toBeInTheDocument();
  expect(screen.getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual(["Tóm tắt", "Việc cần làm (2)", "Chương (2)", "Hỏi"]);
  expect(headings()).toEqual(["Tóm tắt", "Ý chính", "Quyết định", "Câu hỏi còn mở"]);

  await user.click(screen.getByRole("tab", { name: "Việc cần làm (2)" }));
  expect(headings()).toEqual(["Việc cần làm"]);
  expect(screen.getByRole("button", { name: "Copy dạng checklist" })).toBeInTheDocument();

  await user.click(screen.getByRole("tab", { name: "Chương (2)" }));
  expect(headings()).toEqual(["Chương"]);

  await user.click(screen.getByRole("tab", { name: "Hỏi" }));
  expect(headings()).toEqual(["Hỏi đáp"]);
});

// ---- Every line of the summary has its moment

/** The summary of a recording that was made with the prompt that asks for a time on each item: the time ends the text. */
function timed(): MeetingDetail["summary"] {
  return fakeSummary({
    keyPoints: "- Budget is **fixed** [01:05]\n- Hiring starts in May [1:02:03]",
    decisions: ["Ship in May [02:10]"],
    actions: [
      { index: 0, owner: "An", text: "Send the budget (Friday) [03:20]", done: false },
      { index: 1, owner: null, text: "Book the room [04:30]", done: false },
    ],
    questions: ["Who owns the docs? [05:40]"],
  });
}

const styles = parseCss(styleOf(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "SummaryPane.svelte"), "utf8")));

test("a_trailing_time_is_a_chip_that_seeks: the time at the end of each kind of item is a link to its moment, and the text keeps it", async () => {
  const user = userEvent.setup();
  pane({ summary: timed() });

  // Each kind of item: the tab and the part it is in, the time at its end, and the second that it seeks to.
  const ends: [string, string, string, number][] = [
    ["Summary", "Key points", "[01:05]", 65],
    ["Summary", "Key points", "[1:02:03]", 3723],
    ["Summary", "Decisions", "[02:10]", 130],
    ["Actions", "Action items", "[03:20]", 200],
    ["Actions", "Action items", "[04:30]", 270],
    ["Summary", "Open questions", "[05:40]", 340],
  ];
  for (const [view, name, stamp, at] of ends) {
    await user.click(tab(view));
    const link = within(part(name)).getByRole("link", { name: stamp });
    expect(link, `${stamp} ends its item`).toHaveAttribute("data-seek", String(at));
    // At the end of the text of its item: nothing follows it on its line.
    expect((link.nextSibling?.textContent ?? "").split("\n")[0], `${stamp} is last on its line`).toBe("");
    await user.click(link);
    expect(onseek).toHaveBeenLastCalledWith(at);
  }
  expect(onseek).toHaveBeenCalledTimes(ends.length);

  // The time is in the text of the action, and the owner is the owner's alone; ticking is not seeking.
  await user.click(tab("Actions"));
  const item = screen.getByRole("checkbox", { name: "Send the budget (Friday) [03:20]" });
  expect(item).not.toBeChecked();
  expect(screen.getByText("An")).toHaveClass("owner");
  expect(screen.getByText("An").textContent).toBe("An");
  expect(api.meetingSetTask).not.toHaveBeenCalled();
  // An item with no owner has the time in its text and no owner at all.
  expect(screen.getByRole("checkbox", { name: "Book the room [04:30]" })).toBeInTheDocument();
  expect(document.querySelectorAll(".owner")).toHaveLength(1);
});

test("a_trailing_time_is_a_chip_that_seeks: the link of a time is drawn as a pill in the timestamp color and the monospaced face", () => {
  const chip = declarationsOf(styles, ".sections :global(.inline a[data-seek])");
  expect(chip["border-radius"]).toBe("999px");
  expect(chip.color).toBe("var(--ts)");
  expect(chip["font-family"]).toBe("var(--mono)");
  expect(chip["font-variant-numeric"]).toBe("tabular-nums");
  expect(chip.background).toBe("var(--panel)");
  expect(chip.border).toBe("1px solid var(--line)");
  expect(declarationsOf(styles, ".sections :global(.inline a[data-seek]:hover)").background).toBe("var(--hl)");
});

test("a_trailing_time_is_a_chip_that_seeks: with no player the time is plain text and is not drawn as a pill", async () => {
  const user = userEvent.setup();
  pane({ summary: timed() }, false);

  for (const view of ["Summary", "Actions"]) {
    await user.click(tab(view));
    expect(document.querySelectorAll(".sections a[data-seek]").length, `${view} has times`).toBeGreaterThan(0);
    for (const link of document.querySelectorAll(".sections a[data-seek]")) expect(link).toHaveAttribute("data-inert");
  }
  // The pill is left off an inert one: it is text again, as it was in the summary.
  const inert = declarationsOf(styles, ".sections :global(.inline a[data-inert])");
  expect(inert.border).toBe("0");
  expect(inert.background).toBe("none");
  expect(inert.color).toBe("inherit");
});

/** What a selector of a Svelte style weighs on the page, as [classes, attributes and pseudo-classes, types]. Svelte adds the class of the component once. */
function weight(selector: string): [number, number] {
  const plain = selector.replace(/:global\((.*)\)$/, "$1");
  const classes = (plain.match(/\.[\w-]+/g) ?? []).length + (plain.match(/\[[^\]]*\]/g) ?? []).length + (plain.match(/(?<!:):[\w-]+/g) ?? []).length;
  const types = (plain.match(/(?:^|[\s>+~])[a-z][\w-]*/g) ?? []).length;
  return [classes + 1, types];
}

const heavier = (a: [number, number], b: [number, number]) => a[0] > b[0] || (a[0] === b[0] && a[1] > b[1]);

test("a_trailing_time_is_a_chip_that_seeks: the chip wins over the style of a stamp in Inline whatever the order of the style sheets", () => {
  const inline = parseCss(styleOf(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "Inline.svelte"), "utf8")));
  const own = (rules: typeof styles, property: string) =>
    rules.flatMap((rule) => (property in rule.declarations ? rule.selectors.map((selector) => ({ selector, rule })) : []));

  // Every property that both set (the size, the color and the face) is set by the chip with a selector that weighs more, so that the
  // order of the two style sheets in the page does not decide.
  const chipSelector = ".sections :global(.inline a[data-seek])";
  const stamp = ".inline :global(a[data-seek])";
  expect(inline.some((rule) => rule.selectors.includes(stamp))).toBe(true);
  expect(heavier(weight(chipSelector), weight(stamp))).toBe(true);
  for (const property of ["font-size", "color", "font-family"]) {
    expect(own(styles, property).some(({ selector }) => selector === chipSelector), `the chip sets ${property}`).toBe(true);
    expect(own(inline, property).some(({ selector }) => selector === stamp), `a stamp sets ${property}`).toBe(true);
  }
  expect(declarationsOf(styles, chipSelector)["font-size"]).toBe("0.85em");

  // The stamp that has no player is plain text again: as heavy as the chip, and after it, so that it wins over the chip (and over
  // the stamp of Inline, which it is heavier than).
  const inert = ".sections :global(.inline a[data-inert])";
  expect(weight(inert)).toEqual(weight(chipSelector));
  expect(heavier(weight(inert), weight(stamp))).toBe(true);
  const order = styles.flatMap((rule) => rule.selectors);
  expect(order.indexOf(inert)).toBeGreaterThan(order.indexOf(chipSelector));
});

test("a_trailing_time_is_a_chip_that_seeks: a summary from before the times shows as it was, with no link of its own", async () => {
  const user = userEvent.setup();
  pane({
    summary: fakeSummary({ keyPoints: "- Budget is fixed", decisions: ["Ship in May"], questions: ["Who owns the docs?"], actions: [{ index: 0, owner: null, text: "Book the room", done: false }] }),
  });

  expect(document.querySelectorAll(".sections a[data-seek]")).toHaveLength(0);
  expect(screen.getByText("Ship in May")).toBeInTheDocument();
  await user.click(tab("Actions"));
  expect(document.querySelectorAll(".sections a[data-seek]")).toHaveLength(0);
});

// ---- The tabs

test("tabs_show_counts_and_hide_zero: Actions and Chapters say how many they have, and say nothing when they have none", () => {
  pane();
  expect(screen.getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual(["Summary", "Actions (2)", "Chapters (2)", "Ask"]);
  cleanup();

  pane({ summary: fakeSummary({ actions: [], timeline: [{ seconds: 0, label: "00:00", text: "Kick-off" }] }) });
  expect(screen.getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual(["Summary", "Actions", "Chapters (1)", "Ask"]);
  // The name that a screen reader gets is the same: "Actions" and no "(0)".
  expect(screen.getByRole("tab", { name: "Actions" })).toBeInTheDocument();
  expect(screen.queryByText(/\(0\)/)).not.toBeInTheDocument();
});

test("the count of the actions is of all of them, so that a tick does not change it", async () => {
  const user = userEvent.setup();
  pane({}, true, "actions");

  await user.click(screen.getByRole("checkbox", { name: "Send the budget" }));

  expect(screen.getByRole("tab", { name: "Actions (2)" })).toBeInTheDocument();
});

test("the_tabs_are_an_ARIA_tablist: one tab is selected and tabbable, the panel is named by it, and the arrow keys move between them", async () => {
  const user = userEvent.setup();
  pane();
  const list = screen.getByRole("tablist", { name: "Recording sections" });
  const tabs = within(list).getAllByRole("tab");
  const [summary, actions, chapters, ask] = tabs;

  // Only the selected tab is in the order of the Tab key; the others are reached by the arrows.
  expect(tabs.map((item) => item.getAttribute("aria-selected"))).toEqual(["true", "false", "false", "false"]);
  expect(tabs.map((item) => item.tabIndex)).toEqual([0, -1, -1, -1]);
  const panel = screen.getByRole("tabpanel");
  expect(panel).toHaveAttribute("aria-labelledby", summary.id);
  expect(summary).toHaveAttribute("aria-controls", panel.id);
  expect(new Set(tabs.map((item) => item.id)).size).toBe(4);

  summary.focus();
  await user.keyboard("{ArrowRight}");
  expect(actions).toHaveFocus();
  expect(actions).toHaveAttribute("aria-selected", "true");
  expect(actions.tabIndex).toBe(0);
  expect(summary.tabIndex).toBe(-1);
  expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", actions.id);
  expect(headings()).toEqual(["Action items"]);

  await user.keyboard("{End}");
  expect(ask).toHaveFocus();
  expect(ask).toHaveAttribute("aria-selected", "true");

  // It goes round: right of the last is the first, left of the first is the last.
  await user.keyboard("{ArrowRight}");
  expect(summary).toHaveFocus();
  await user.keyboard("{ArrowLeft}");
  expect(ask).toHaveFocus();
  await user.keyboard("{ArrowLeft}");
  expect(chapters).toHaveFocus();
  await user.keyboard("{Home}");
  expect(summary).toHaveFocus();
  expect(headings()).toEqual(["Summary", "Key points", "Decisions", "Open questions"]);
});

test("the tabs keep their words whole in a narrow column: the row scrolls sideways, and a tab neither shrinks nor wraps", () => {
  const row = declarationsOf(styles, ".tabs");
  expect(row.display).toBe("flex");
  expect(row["overflow-x"]).toBe("auto");
  const tab = declarationsOf(styles, ".tab");
  expect(tab.flex).toBe("none");
  expect(tab["white-space"]).toBe("nowrap");
  // The open tab is marked with the accent and the ink, which the theme has in both of its modes.
  const open = declarationsOf(styles, '.tab[aria-selected="true"]');
  expect(open.color).toBe("var(--ink)");
  expect(open["box-shadow"]).toBe("inset 0 -2px 0 var(--accent)");
});

test("the panel is in the order of the Tab key, so that a keyboard can scroll it", () => {
  pane();
  expect(screen.getByRole("tabpanel").tabIndex).toBe(0);
});

test("the_last_tab_is_remembered: a tab that is chosen is kept, and the pane of the next recording opens on it", async () => {
  const user = userEvent.setup();
  const first = pane();

  await user.click(tab("Chapters"));
  expect(localStorage.getItem(TAB_KEY)).toBe("chapters");

  first.unmount();
  render(SummaryPane, { store, detail: fakeDetail({ summary: fakeSummary() }), canSeek: true, onseek });
  expect(tab("Chapters")).toHaveAttribute("aria-selected", "true");
  expect(headings()).toEqual(["Chapters"]);
});

test("the_last_tab_is_remembered: a value that is not a tab is ignored, and a store that refuses does not break the pane", async () => {
  const user = userEvent.setup();
  localStorage.setItem(TAB_KEY, "gigantic");
  render(SummaryPane, { store, detail: fakeDetail({ summary: fakeSummary() }), canSeek: true, onseek });
  expect(tab("Summary")).toHaveAttribute("aria-selected", "true");
  cleanup();

  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
    throw new DOMException("denied", "SecurityError");
  });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new DOMException("denied", "QuotaExceededError");
  });
  render(SummaryPane, { store, detail: fakeDetail({ summary: fakeSummary() }), canSeek: true, onseek });
  expect(tab("Summary")).toHaveAttribute("aria-selected", "true");

  // It holds while the pane is shown, though it can't be kept.
  await user.click(tab("Actions"));
  expect(tab("Actions")).toHaveAttribute("aria-selected", "true");
  expect(headings()).toEqual(["Action items"]);
});

test("a remembered tab that this recording has no place for gives way to the Summary: Ask without a transcript, Actions without a summary", () => {
  pane({ transcript: [] }, true, "ask");
  expect(tab("Summary")).toHaveAttribute("aria-selected", "true");
  cleanup();

  pane({ summary: null }, true, "actions");
  expect(tab("Summary")).toHaveAttribute("aria-selected", "true");
  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  // The choice that was made is not overwritten by a recording that could not have it.
  expect(localStorage.getItem(TAB_KEY)).toBe("actions");
});

// ---- The overview

/** A box of text that is taller than its three lines: what a web view reports of a long overview that is clamped. */
function overflowing(on: boolean) {
  vi.spyOn(Element.prototype, "scrollHeight", "get").mockReturnValue(on ? 120 : 60);
  vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(60);
}

test("the_overview_is_clamped_with_show_more: a long overview is cut at three lines, and Show more and Show less toggle it", async () => {
  const user = userEvent.setup();
  overflowing(true);
  pane();
  const overview = within(part("Summary")).getByText("We planned the quarter.").closest(".overview") as HTMLElement;

  expect(overview).toHaveClass("clamped");
  const more = screen.getByRole("button", { name: "Show more" });
  expect(more).toHaveAttribute("aria-expanded", "false");
  expect(more).toHaveAttribute("aria-controls", overview.id);

  await user.click(more);

  expect(overview).not.toHaveClass("clamped");
  const less = screen.getByRole("button", { name: "Show less" });
  expect(less).toHaveAttribute("aria-expanded", "true");
  expect(screen.queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();

  await user.click(less);
  expect(overview).toHaveClass("clamped");
  expect(screen.getByRole("button", { name: "Show more" })).toBeInTheDocument();
});

test("the_overview_is_clamped_with_show_more: an overview that fits in three lines has no button", () => {
  overflowing(false);
  pane();

  expect(screen.queryByRole("button", { name: /Show (more|less)/ })).not.toBeInTheDocument();
});

test("the_overview_is_clamped_with_show_more: the clamp is three lines, and the other parts of the Summary tab are not clamped", () => {
  const overview = parseCss(styleOf(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "Overview.svelte"), "utf8")));
  const clamp = declarationsOf(overview, ".overview.clamped");
  expect(clamp["-webkit-line-clamp"]).toBe("3");
  expect(clamp["line-clamp"]).toBe("3");
  expect(clamp.overflow).toBe("hidden");
  expect(clamp.display).toBe("-webkit-box");
  expect(JSON.stringify(styles)).not.toContain("line-clamp");
});

test("the overview is in the words of the window, and the stamps in it still seek", async () => {
  const user = userEvent.setup();
  overflowing(true);
  setLanguage("vi");
  pane({ summary: fakeSummary({ summary: "Họp lúc [00:30] về quý mới." }) });

  await user.click(screen.getByRole("link", { name: "[00:30]" }));
  expect(onseek).toHaveBeenCalledWith(30);
  await user.click(screen.getByRole("button", { name: "Xem thêm" }));
  expect(screen.getByRole("button", { name: "Thu gọn" })).toBeInTheDocument();
});

// ---- A short recording

/** A transcript of `words` words in one line. */
const spoken = (words: number) => [{ start: 0, stamp: "00:00", speaker: null, text: Array.from({ length: words }, () => "word").join(" ") }];
const SHORT = "Short recording — there isn't much to summarize.";

test("a_short_recording_says_so: under a minute, or under 40 words, the Summary tab has one quiet line", () => {
  // A minute and more, and enough to say: nothing to add.
  pane({ row: fakeRow({ duration: 60 }), transcript: spoken(40) });
  expect(screen.queryByText(SHORT)).not.toBeInTheDocument();
  cleanup();

  // Under a minute, with enough words.
  pane({ row: fakeRow({ duration: 59 }), transcript: spoken(400) });
  expect(screen.getByText(SHORT)).toBeInTheDocument();
  cleanup();

  // Long enough, with little said.
  pane({ row: fakeRow({ duration: 3600 }), transcript: spoken(39) });
  expect(screen.getByText(SHORT)).toBeInTheDocument();
});

test("a_short_recording_says_so: the length is not known, or there is no transcript: nothing says it is short", () => {
  // A length that is not known counts for nothing; the words decide.
  pane({ row: fakeRow({ duration: null }), transcript: spoken(100) });
  expect(screen.queryByText(SHORT)).not.toBeInTheDocument();
  cleanup();

  // No transcript is not a short one: it may be on its way, or have failed.
  pane({ row: fakeRow({ duration: 3600 }), transcript: [] });
  expect(screen.queryByText(SHORT)).not.toBeInTheDocument();
});

test("a_short_recording_says_so: the line is quiet, in the Summary tab only, and Vietnamese in Vietnamese", async () => {
  const user = userEvent.setup();
  pane({ row: fakeRow({ duration: 20 }) });

  const note = screen.getByText(SHORT);
  expect(note).toHaveClass("hint");
  expect(screen.getByRole("tabpanel")).toContainElement(note);
  expect(screen.getAllByRole("heading", { level: 2 }).some((heading) => heading.contains(note))).toBe(false);
  await user.click(tab("Actions"));
  expect(screen.queryByText(SHORT)).not.toBeInTheDocument();
  cleanup();

  setLanguage("vi");
  pane({ row: fakeRow({ duration: 20 }) });
  expect(screen.getByText("Bản ghi ngắn — không có nhiều nội dung để tóm tắt.")).toBeInTheDocument();
});

test("a_short_recording_says_so: a short recording with no summary yet says it too", () => {
  pane({ row: fakeRow({ duration: 20 }), summary: null });

  expect(screen.getByText("No summary yet")).toBeInTheDocument();
  expect(screen.getByText(SHORT)).toBeInTheDocument();
});
