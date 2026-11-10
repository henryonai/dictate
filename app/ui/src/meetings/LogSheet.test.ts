import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { MeetingsStore } from "../lib/meetings-store.svelte";
import { fakeRow, stubMeetingsApi } from "../test-helpers";
import LogSheet from "./LogSheet.svelte";

const ID = "2026-03-02 10-00-00";
let store: MeetingsStore;

beforeEach(async () => {
  stubMeetingsApi([fakeRow({ id: ID, title: "Weekly planning" })]);
  store = new MeetingsStore();
  await store.start();
});

afterEach(() => {
  store.stop();
  vi.restoreAllMocks();
  setLanguage("en");
});

test("is not there until a log is opened", () => {
  render(LogSheet, { store });

  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(api.meetingLog).not.toHaveBeenCalled();
});

test("shows the log of the recording under its title, in a sheet that has Copy and Close", async () => {
  vi.mocked(api.meetingLog).mockResolvedValue("Downloading audio from https://youtu.be/x…\nERROR: yt-dlp failed");
  render(LogSheet, { store });

  store.showLog(ID);

  const sheet = await screen.findByRole("dialog", { name: "Log — Weekly planning" });
  expect(api.meetingLog).toHaveBeenCalledWith(ID);
  expect(await within(sheet).findByText(/Downloading audio from https:\/\/youtu\.be\/x…/)).toBeInTheDocument();
  expect(within(sheet).getByText(/ERROR: yt-dlp failed/)).toBeInTheDocument();
  expect(within(sheet).getByRole("button", { name: "Copy" })).toBeInTheDocument();
  expect(within(sheet).getByRole("button", { name: "Close" })).toBeInTheDocument();
});

test("Copy puts the whole log on the clipboard", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingLog).mockResolvedValue("line one\nline two");
  render(LogSheet, { store });
  store.showLog(ID);
  await screen.findByText(/line one/);

  await user.click(screen.getByRole("button", { name: "Copy" }));

  expect(api.copyText).toHaveBeenCalledWith("line one\nline two");
});

test("Copy says that it has copied, for a moment", async () => {
  vi.mocked(api.meetingLog).mockResolvedValue("line one");
  render(LogSheet, { store });
  store.showLog(ID);
  await screen.findByText(/line one/);
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });

  await fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  await vi.advanceTimersByTimeAsync(0);
  expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();

  await vi.advanceTimersByTimeAsync(2000);
  expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  vi.useRealTimers();
});

test("a copy that failed does not say that it copied", async () => {
  vi.mocked(api.meetingLog).mockResolvedValue("line one");
  vi.mocked(api.copyText).mockRejectedValue("The clipboard is busy.");
  render(LogSheet, { store });
  store.showLog(ID);
  await screen.findByText(/line one/);

  await fireEvent.click(screen.getByRole("button", { name: "Copy" }));

  await vi.waitFor(() => expect(store.notice).toBe("The clipboard is busy."));
  expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
});

test("a press that starts in the sheet and ends on the backdrop (a selection that was dragged) does not close it", async () => {
  vi.mocked(api.meetingLog).mockResolvedValue("some log");
  render(LogSheet, { store });
  store.showLog(ID);
  const sheet = await screen.findByRole("dialog");
  await screen.findByText("some log");
  const backdrop = sheet.parentElement as HTMLElement;

  await fireEvent.pointerDown(screen.getByText("some log"));
  await fireEvent.click(backdrop);
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(store.logFor).toBe(ID);

  // A press that is on the backdrop from start to end closes it.
  await fireEvent.pointerDown(backdrop);
  await fireEvent.click(backdrop);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(store.logFor).toBeNull();
});

test("an empty log says so, and has nothing to copy", async () => {
  vi.mocked(api.meetingLog).mockResolvedValue("");
  render(LogSheet, { store });

  store.showLog(ID);

  expect(await screen.findByText("Empty.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
});

test("a log that can't be read says why", async () => {
  vi.mocked(api.meetingLog).mockRejectedValue("The recording was not found.");
  render(LogSheet, { store });

  store.showLog(ID);

  expect(await screen.findByRole("alert")).toHaveTextContent("The recording was not found.");
});

test("Close and Esc close it", async () => {
  const user = userEvent.setup();
  render(LogSheet, { store });

  store.showLog(ID);
  await user.click(await screen.findByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(store.logFor).toBeNull();

  store.showLog(ID);
  await screen.findByRole("dialog");
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("takes the focus when it opens and gives it back when it closes", async () => {
  const user = userEvent.setup();
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  render(LogSheet, { store });

  store.showLog(ID);
  const sheet = await screen.findByRole("dialog");
  await vi.waitFor(() => expect(sheet).toContainElement(document.activeElement as HTMLElement));

  await user.keyboard("{Escape}");
  await vi.waitFor(() => expect(document.activeElement).toBe(opener));
  opener.remove();
});

test("Tab goes round the buttons of the sheet and not out of it", async () => {
  const user = userEvent.setup();
  vi.mocked(api.meetingLog).mockResolvedValue("some log");
  // Buttons of the page behind the sheet: the focus must not reach them.
  const behind = [document.createElement("button"), document.createElement("button")];
  document.body.prepend(behind[0]);
  document.body.append(behind[1]);
  render(LogSheet, { store });
  store.showLog(ID);
  await screen.findByText("some log");
  const copy = screen.getByRole("button", { name: "Copy" });
  const close = screen.getByRole("button", { name: "Close" });
  await vi.waitFor(() => expect(document.activeElement).toBe(close));

  await user.tab();
  expect(document.activeElement).toBe(copy);

  await user.tab({ shift: true });
  expect(document.activeElement).toBe(close);
  await user.tab({ shift: true });
  expect(document.activeElement).toBe(copy);
  behind.forEach((button) => button.remove());
});

test("the log of another recording is read when it is opened", async () => {
  vi.mocked(api.meetingLog).mockImplementation(async (id) => `log of ${id}`);
  render(LogSheet, { store });

  store.showLog(ID);
  expect(await screen.findByText(`log of ${ID}`)).toBeInTheDocument();
  store.showLog("2026-03-01 09-00-00");

  expect(await screen.findByText("log of 2026-03-01 09-00-00")).toBeInTheDocument();
});

test("a log that looks like HTML is shown as text", async () => {
  vi.mocked(api.meetingLog).mockResolvedValue('<img src=x onerror="alert(1)">');
  const { container } = render(LogSheet, { store });

  store.showLog(ID);

  expect(await screen.findByText('<img src=x onerror="alert(1)">')).toBeInTheDocument();
  expect(container.querySelector("img")).toBeNull();
});

test("speaks Vietnamese when the language is", async () => {
  setLanguage("vi");
  vi.mocked(api.meetingLog).mockResolvedValue("");
  render(LogSheet, { store });

  store.showLog(ID);

  expect(await screen.findByRole("dialog", { name: "Log — Weekly planning" })).toBeInTheDocument();
  expect(screen.getByText("Trống.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Đóng" })).toBeInTheDocument();
});
