import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import type { MeetingDetail, SpeakerView } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { PlayerController } from "../lib/player.svelte";
import { FakeMedia, fakeDetail, fakeError, fakeMedia, fakeRow, fakeSummary, stubMeetingsApi } from "../test-helpers";
import SpeakerSheet from "./SpeakerSheet.svelte";

const ID = "2026-03-02 10-00-00";
const LONG = "First, the budget for the next quarter has to be fixed before we hire anyone.";

/** Three speakers in the order that they first talk: the one that is last in the alphabet comes first, so that a sort by name shows. */
const SPEAKERS: SpeakerView[] = [
  { name: "Zed", lines: 3, sample: { start: 65, stamp: "01:05", speaker: "Zed", text: LONG } },
  { name: "Speaker 2", lines: 1, sample: { start: 12, stamp: "00:12", speaker: "Speaker 2", text: "Yes." } },
  { name: "Speaker 3", lines: 2, sample: { start: 90, stamp: "01:30", speaker: "Speaker 3", text: "Sounds good to me, thank you." } },
];

let events: ReturnType<typeof stubMeetingsApi>;
let store: MeetingsStore;
let element: FakeMedia;
let player: PlayerController;

/** A recording with three speakers, a summary and a player that works, unless a test says otherwise. */
function whole(overrides: Partial<MeetingDetail> = {}): MeetingDetail {
  return fakeDetail({
    row: fakeRow({ id: ID }),
    speakers: SPEAKERS,
    summary: fakeSummary(),
    media: fakeMedia(),
    ...overrides,
  });
}

/** The sheet over a recording, asked for as the transcript or the menu ask for it (`speaker` is the one whose name was clicked). */
async function opened(detail: MeetingDetail = whole(), speaker?: string, language: "en" | "vi" = "en") {
  events = stubMeetingsApi([fakeRow({ id: ID })]);
  // The window speaks what the settings say: the store takes it from what is ready.
  events.setReadiness({ ai: true, whisper: true, language });
  store = new MeetingsStore();
  await store.start();
  player = new PlayerController(async () => detail.media);
  player.bind(ID, detail.media);
  element = new FakeMedia();
  player.attach(element as unknown as HTMLMediaElement);
  store.openRename(speaker);
  const view = render(SpeakerSheet, { store, detail, player });
  return { user: userEvent.setup(), view, dialog: await screen.findByRole("dialog") };
}

const rows = (dialog: HTMLElement) => within(dialog).getAllByRole("listitem");
const field = (name: string, label = "New name for") => screen.getByRole("textbox", { name: `${label} ${name}` });
const campo = (name: string) => field(name, "Tên mới cho");
const rename = () => screen.getByRole("button", { name: "Rename" });

beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
});

afterEach(() => {
  store?.stop();
  vi.restoreAllMocks();
  setLanguage("en");
});

test("speaker_sheet_lists_in_order_with_samples: the speakers as they first talk, each with its lines, a quote and a field", async () => {
  const { dialog } = await opened();

  expect(within(dialog).getByText(
    "New names replace the labels in the transcript and the summary, without calling AI. Give two speakers the same name to merge them.",
  )).toBeInTheDocument();
  const list = rows(dialog);
  expect(list).toHaveLength(3);
  expect(list.map((row) => within(row).getByRole("textbox").getAttribute("aria-label"))).toEqual([
    "New name for Zed",
    "New name for Speaker 2",
    "New name for Speaker 3",
  ]);
  const first = within(list[0]);
  expect(first.getByText("Zed")).toBeInTheDocument();
  expect(first.getByText("3 lines")).toBeInTheDocument();
  expect(first.getByText(`“${LONG}”`)).toBeInTheDocument();
  expect(first.getByRole("textbox")).toHaveAttribute("placeholder", "New name");
  // One line is "1 line".
  expect(within(list[1]).getByText("1 line")).toBeInTheDocument();
  expect(within(list[1]).getByText("“Yes.”")).toBeInTheDocument();
  expect(within(list[2]).getByText("2 lines")).toBeInTheDocument();
});

test("a speaker name and a sample that are made of HTML are shown as text", async () => {
  const sample = '<img src=x onerror="document.title=1"> <script>document.title=2</script>';
  const detail = whole({
    speakers: [{ name: "<b>Eve</b>", lines: 2, sample: { start: 1, stamp: "00:01", speaker: "<b>Eve</b>", text: sample } }],
  });

  const { dialog } = await opened(detail);

  expect(within(dialog).getByText("<b>Eve</b>")).toBeInTheDocument();
  expect(within(dialog).getByText(`“${sample}”`)).toBeInTheDocument();
  expect(dialog.querySelector("img, script, b")).toBeNull();
});

test("the field of the speaker whose name was clicked has the focus, and the first one when the sheet was asked for as a whole", async () => {
  const clicked = await opened(whole(), "Speaker 3");
  expect(field("Speaker 3")).toHaveFocus();
  clicked.view.unmount();

  await opened(whole());
  expect(field("Zed")).toHaveFocus();
});

test("Listen goes to the start of the sample in the main player, and plays", async () => {
  const { user } = await opened();

  await user.click(screen.getByRole("button", { name: "Listen to Speaker 3 from 01:30" }));

  await vi.waitFor(() => expect(element.play).toHaveBeenCalled());
  expect(element.currentTime).toBe(90);
});

test("Listen is off, and says why, when there is no video or audio", async () => {
  const { dialog } = await opened(whole({ media: null }));

  for (const button of within(dialog).getAllByRole("button", { name: /^Listen to / })) {
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "No video or audio to listen to");
  }
});

test("a player that has failed cannot be listened to either", async () => {
  const { dialog } = await opened();
  player.failed = true;

  await vi.waitFor(() => expect(within(dialog).getByRole("button", { name: "Listen to Zed from 01:05" })).toBeDisabled());
});

test("speaker_sheet_rename_disabled_without_change: Rename waits for a name that is a change", async () => {
  const { user } = await opened();
  expect(rename()).toBeDisabled();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();

  await user.type(field("Speaker 2"), "   ");
  expect(rename()).toBeDisabled();

  // The name that it already has is no change; neither is one that is left with nothing.
  await user.clear(field("Speaker 2"));
  await user.type(field("Speaker 2"), "Speaker 2");
  expect(rename()).toBeDisabled();
  await user.clear(field("Speaker 2"));
  await user.type(field("Speaker 2"), ":.*");
  expect(rename()).toBeDisabled();

  await user.clear(field("Speaker 2"));
  await user.type(field("Speaker 2"), "An");
  expect(rename()).toBeEnabled();

  await user.clear(field("Speaker 2"));
  expect(rename()).toBeDisabled();
});

test("speaker_sheet_merge_warning: two speakers given one name are said to become one", async () => {
  const { user, dialog } = await opened();
  expect(within(dialog).queryByText(/will become one speaker/)).not.toBeInTheDocument();

  await user.type(field("Speaker 2"), "An");
  expect(within(dialog).queryByText(/will become one speaker/)).not.toBeInTheDocument();

  await user.type(field("Speaker 3"), "An");
  expect(within(dialog).getByText("Speaker 2 + Speaker 3 will become one speaker: “An”.")).toBeInTheDocument();
  expect(rename()).toBeEnabled();

  // A speaker given the name of one that stays is merged with it.
  await user.clear(field("Speaker 3"));
  await user.clear(field("Speaker 2"));
  await user.type(field("Speaker 2"), "Zed");
  expect(within(dialog).getByText("Zed + Speaker 2 will become one speaker: “Zed”.")).toBeInTheDocument();

  await user.clear(field("Speaker 2"));
  expect(within(dialog).queryByText(/will become one speaker/)).not.toBeInTheDocument();
});

test("a swap of two names is no merge", async () => {
  const { user, dialog } = await opened();

  await user.type(field("Speaker 2"), "Speaker 3");
  await user.type(field("Speaker 3"), "Speaker 2");

  expect(within(dialog).queryByText(/will become one speaker/)).not.toBeInTheDocument();
  expect(rename()).toBeEnabled();
});

test("a name that is too long, or has a colon, a dot or an asterisk, is told that the rest is dropped", async () => {
  const { user, dialog } = await opened();
  const hint = "Names have at most 24 characters and no “:”, “.” or “*”: the rest is dropped.";
  expect(within(dialog).queryByText(hint)).not.toBeInTheDocument();

  await user.type(field("Speaker 2"), "Dr. An");
  expect(within(dialog).getByText(hint)).toBeInTheDocument();

  await user.clear(field("Speaker 2"));
  await user.type(field("Speaker 2"), "Chị Lan");
  expect(within(dialog).queryByText(hint)).not.toBeInTheDocument();

  await user.clear(field("Speaker 2"));
  await user.type(field("Speaker 2"), "a".repeat(30));
  expect(within(dialog).getByText(hint)).toBeInTheDocument();
});

test("speaker_sheet_resummarize_toggle_only_with_summary_and_ai", async () => {
  const toggle = () => screen.queryByRole("checkbox", { name: "Summarize again with the new names (calls AI)" });
  const cases: [string, MeetingDetail, boolean, boolean][] = [
    ["a summary and the AI", whole(), true, true],
    ["a summary and no AI", whole(), false, false],
    ["the AI and no summary", whole({ summary: null }), true, false],
    ["no summary and no AI", whole({ summary: null }), false, false],
  ];

  for (const [name, detail, ai, shown] of cases) {
    const { view } = await opened(detail);
    events.setReadiness({ ai, whisper: true, language: "en" });
    await store.refreshReadiness();

    await vi.waitFor(() => expect(toggle() !== null, name).toBe(shown));
    view.unmount();
    store.stop();
  }
});

test("Rename gives the names that changed, with the choice not to summarize again, and the sheet closes", async () => {
  const { user } = await opened();
  await user.type(field("Speaker 2"), "  chị   Lan ");
  await user.type(field("Speaker 3"), "Speaker 3");

  await user.click(rename());

  await vi.waitFor(() => expect(store.renameFor).toBeNull());
  expect(api.meetingRenameSpeakers).toHaveBeenCalledWith(ID, { "Speaker 2": "chị Lan" }, false);
});

test("Rename with the toggle on asks for the summary to be written again", async () => {
  const { user } = await opened();
  await user.type(field("Zed"), "An");

  await user.click(screen.getByRole("checkbox", { name: "Summarize again with the new names (calls AI)" }));
  await user.click(rename());

  await vi.waitFor(() => expect(api.meetingRenameSpeakers).toHaveBeenCalledWith(ID, { Zed: "An" }, true));
});

test("Enter in a field gives the names, as Rename does, and does nothing while there is no change", async () => {
  const { user } = await opened();

  await user.type(field("Zed"), "{Enter}");
  expect(api.meetingRenameSpeakers).not.toHaveBeenCalled();

  await user.type(field("Zed"), "An{Enter}");
  await vi.waitFor(() => expect(api.meetingRenameSpeakers).toHaveBeenCalledWith(ID, { Zed: "An" }, false));
});

test("a submit that comes without a change, or while a rename is on, gives nothing: only the button's own state is not enough", async () => {
  const { user, dialog } = await opened();
  const form = dialog.querySelector("form") as HTMLFormElement;

  await fireEvent.submit(form);
  expect(api.meetingRenameSpeakers).not.toHaveBeenCalled();

  let finish: () => void = () => {};
  vi.mocked(api.meetingRenameSpeakers).mockImplementation(() => new Promise((resolve) => (finish = () => resolve([]))));
  await user.type(field("Zed"), "An");
  await fireEvent.submit(form);
  await fireEvent.submit(form);
  expect(api.meetingRenameSpeakers).toHaveBeenCalledTimes(1);
  finish();
  await vi.waitFor(() => expect(store.renameFor).toBeNull());
});

test("the choice to summarize again is not sent when the AI went away after it was made", async () => {
  const { user } = await opened();
  await user.type(field("Zed"), "An");
  await user.click(screen.getByRole("checkbox", { name: "Summarize again with the new names (calls AI)" }));
  events.setReadiness({ ai: false, whisper: true, language: "en" });
  await store.refreshReadiness();
  await vi.waitFor(() => expect(screen.queryByRole("checkbox", { name: /Summarize again/ })).not.toBeInTheDocument());

  await user.click(rename());

  await vi.waitFor(() => expect(api.meetingRenameSpeakers).toHaveBeenCalledWith(ID, { Zed: "An" }, false));
});

test("a rename is not given twice by a second press while it is on", async () => {
  const { user } = await opened();
  let finish: () => void = () => {};
  vi.mocked(api.meetingRenameSpeakers).mockImplementation(() => new Promise((resolve) => (finish = () => resolve([]))));
  await user.type(field("Zed"), "An");

  await user.click(rename());
  await user.click(rename());

  expect(api.meetingRenameSpeakers).toHaveBeenCalledTimes(1);
  expect(rename()).toBeDisabled();
  finish();
  await vi.waitFor(() => expect(store.renameFor).toBeNull());
});

test("a rename that could not be saved says why, as the sheet's own sentence, and the sheet stays with what was typed", async () => {
  const { user, dialog } = await opened();
  await user.type(field("Zed"), "An");
  vi.mocked(api.meetingRenameSpeakers).mockRejectedValue(
    fakeError({ detail: "Permission denied (os error 13)" }),
  );

  await user.click(rename());

  expect((await within(dialog).findByRole("alert")).textContent).toBe("Couldn't save the speaker names: Permission denied (os error 13)");
  expect(store.renameFor).not.toBeNull();
  expect(field("Zed")).toHaveValue("An");
  // It can be tried again.
  expect(rename()).toBeEnabled();
});

test("a failure that has words of its own is said after the sheet's sentence, and a new try takes the old failure away", async () => {
  const { user, dialog } = await opened();
  await user.type(field("Zed"), "An");
  vi.mocked(api.meetingRenameSpeakers).mockRejectedValueOnce(fakeError({ key: "meetings.error.busy", detail: null }));

  await user.click(rename());

  expect((await within(dialog).findByRole("alert")).textContent).toBe(
    "Couldn't save the speaker names: This recording is being processed. Try again when it is done.",
  );

  await user.click(rename());
  await vi.waitFor(() => expect(store.renameFor).toBeNull());
  expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
});

test("Cancel, Esc and the backdrop are off while the names are being saved, so that a save that fails is not lost with the sheet", async () => {
  const { user, dialog } = await opened();
  let fail: (reason: unknown) => void = () => {};
  vi.mocked(api.meetingRenameSpeakers).mockImplementation(() => new Promise((_, reject) => (fail = reject)));
  await user.type(field("Zed"), "An");
  await user.click(rename());
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();

  await user.click(screen.getByRole("button", { name: "Cancel" }));
  await user.keyboard("{Escape}");
  const backdrop = dialog.parentElement as HTMLElement;
  await fireEvent.pointerDown(backdrop);
  await fireEvent.click(backdrop);

  expect(store.renameFor).not.toBeNull();
  expect(screen.getByRole("dialog")).toBeInTheDocument();

  // The save fails: the sheet is still there to say so, with what was typed, and it can be closed again.
  fail(fakeError({ detail: "Permission denied" }));
  expect((await within(dialog).findByRole("alert")).textContent).toBe("Couldn't save the speaker names: Permission denied");
  expect(field("Zed")).toHaveValue("An");
  expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
  await user.keyboard("{Escape}");
  expect(store.renameFor).toBeNull();
});

test("Cancel and Esc close the sheet and change nothing", async () => {
  const { user } = await opened();
  await user.type(field("Zed"), "An");

  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(store.renameFor).toBeNull();

  store.openRename();
  await fireEvent.keyDown(window, { key: "Escape" });
  expect(store.renameFor).toBeNull();
  expect(api.meetingRenameSpeakers).not.toHaveBeenCalled();
});

test("a sample that was played is stopped when the sheet closes, and a player that the sheet did not start is left playing", async () => {
  const listened = await opened();
  await listened.user.click(screen.getByRole("button", { name: "Listen to Zed from 01:05" }));
  await vi.waitFor(() => expect(element.play).toHaveBeenCalled());
  element.pause.mockClear();

  await listened.user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(element.pause).toHaveBeenCalledTimes(1);
  listened.view.unmount();
  store.stop();

  const untouched = await opened();
  element.playAt(30);
  await untouched.user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(element.pause).not.toHaveBeenCalled();
});

test("a sample that was played is stopped when the names are given too", async () => {
  const { user } = await opened();
  await user.click(screen.getByRole("button", { name: "Listen to Zed from 01:05" }));
  await vi.waitFor(() => expect(element.play).toHaveBeenCalled());
  element.pause.mockClear();
  await user.type(field("Zed"), "An");

  await user.click(rename());

  await vi.waitFor(() => expect(store.renameFor).toBeNull());
  expect(element.pause).toHaveBeenCalledTimes(1);
});

test("more than 5 speakers are in a list that scrolls, and 5 or fewer are not", async () => {
  const many = Array.from({ length: 6 }, (_, i): SpeakerView => ({
    name: `Speaker ${i + 1}`,
    lines: 1,
    sample: { start: i, stamp: `00:0${i}`, speaker: `Speaker ${i + 1}`, text: "Hello." },
  }));
  const six = await opened(whole({ speakers: many }));
  expect(six.dialog.querySelector(".speakers")).toHaveClass("scrolls");
  six.view.unmount();
  store.stop();

  const five = await opened(whole({ speakers: many.slice(0, 5) }));
  expect(five.dialog.querySelector(".speakers")).not.toHaveClass("scrolls");
});

test("the words are Vietnamese when the window speaks it", async () => {
  const { dialog } = await opened(whole(), undefined, "vi");

  expect(within(dialog).getByRole("heading", { name: "Đổi tên người nói" })).toBeInTheDocument();
  expect(within(dialog).getByText("3 đoạn")).toBeInTheDocument();
  expect(within(dialog).getByText("1 đoạn")).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Nghe Zed từ 01:05" })).toBeInTheDocument();
  expect(within(dialog).getAllByPlaceholderText("Tên mới")).toHaveLength(3);
  expect(within(dialog).getByRole("checkbox", { name: "Tóm tắt lại theo tên mới (gọi AI)" })).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Huỷ" })).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Đổi tên" })).toBeDisabled();

  const user = userEvent.setup();
  await user.type(campo("Speaker 2"), "Dr. An");
  expect(within(dialog).getByText("Tên tối đa 24 ký tự, không có “:”, “.” hay “*”: phần thừa sẽ được bỏ.")).toBeInTheDocument();
  await user.type(campo("Speaker 3"), "Dr. An");
  expect(within(dialog).getByText("Speaker 2 + Speaker 3 sẽ gộp làm một: “Dr An”.")).toBeInTheDocument();
});

test("Vietnamese words for a player that is not there, and a save that failed", async () => {
  const { user, dialog } = await opened(whole({ media: null }), undefined, "vi");
  expect(within(dialog).getByRole("button", { name: "Nghe Zed từ 01:05" })).toHaveAttribute("title", "Không có video/audio để nghe");

  await user.type(campo("Zed"), "An");
  vi.mocked(api.meetingRenameSpeakers).mockRejectedValue(
    fakeError({ detail: "Permission denied" }),
  );
  await user.click(screen.getByRole("button", { name: "Đổi tên" }));

  expect((await within(dialog).findByRole("alert")).textContent).toBe("Không lưu được tên người nói: Permission denied");
});
