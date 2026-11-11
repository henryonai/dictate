import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { PermissionsView } from "../lib/api";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { stubApi } from "../test-helpers";
import Permissions from "./Permissions.svelte";

const NOT_MAC: PermissionsView = { microphone: "not_needed", accessibility: "not_needed", screen: "not_needed" };

beforeEach(() => {
  stubApi();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  setLanguage("en");
});

const status = (
  microphone: PermissionsView["microphone"],
  accessibility: PermissionsView["accessibility"],
  screen: PermissionsView["screen"] = "granted",
) => vi.mocked(api.permissionsStatus).mockResolvedValue({ microphone, accessibility, screen });

const row = (name: string) => screen.getByRole("listitem", { name });

test("permissions_rows_only_on_macos: the section is there when the system has permissions, and only then", async () => {
  status("granted", "denied");
  const { unmount } = render(Permissions);

  expect(await screen.findByRole("heading", { name: "Permissions" })).toBeInTheDocument();
  expect(row("Microphone")).toBeInTheDocument();
  expect(row("Accessibility")).toBeInTheDocument();
  unmount();

  // Windows and Linux say "not needed" for all of them: nothing is shown, not even the heading.
  vi.mocked(api.permissionsStatus).mockClear();
  vi.mocked(api.permissionsStatus).mockResolvedValue(NOT_MAC);
  render(Permissions);
  await vi.waitFor(() => expect(api.permissionsStatus).toHaveBeenCalled());
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByRole("heading", { name: "Permissions" })).not.toBeInTheDocument();
  expect(screen.queryByRole("listitem")).not.toBeInTheDocument();
});

test("permissions_poll_updates_status: asked again every second while it is on screen, and not any more after it goes", async () => {
  vi.useFakeTimers();
  status("not_determined", "denied");
  const { unmount } = render(Permissions);
  await vi.advanceTimersByTimeAsync(0);
  expect(within(row("Microphone")).getByText("Not asked yet")).toBeInTheDocument();
  expect(within(row("Accessibility")).getByText("Not allowed")).toBeInTheDocument();
  expect(api.permissionsStatus).toHaveBeenCalledTimes(1);

  // The person grants both in System Settings: the next ask shows it, without reloading anything.
  status("granted", "granted");
  await vi.advanceTimersByTimeAsync(999);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(2);
  expect(within(row("Microphone")).getByText("Allowed")).toBeInTheDocument();
  expect(within(row("Accessibility")).getByText("Allowed")).toBeInTheDocument();

  await vi.advanceTimersByTimeAsync(3000);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(5);
  unmount();
  await vi.advanceTimersByTimeAsync(5000);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(5);
});

test("permissions_poll_waits_while_its_tab_is_hidden: Settings keeps every page, and a page nobody sees doesn't ask", async () => {
  vi.useFakeTimers();
  status("granted", "granted");
  // A tab that isn't selected is a hidden panel around the page.
  const tab = document.createElement("div");
  tab.hidden = true;
  document.body.append(tab);
  render(Permissions, { target: tab.appendChild(document.createElement("div")) });
  await vi.advanceTimersByTimeAsync(0);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(1);

  await vi.advanceTimersByTimeAsync(4000);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(1);

  tab.hidden = false;
  await vi.advanceTimersByTimeAsync(1000);
  expect(api.permissionsStatus).toHaveBeenCalledTimes(2);
  tab.remove();
});

test("a microphone nobody was asked about has Grant access, which shows the system's prompt", async () => {
  const user = userEvent.setup();
  status("not_determined", "granted");
  render(Permissions);
  const microphone = await screen.findByRole("listitem", { name: "Microphone" });

  await user.click(within(microphone).getByRole("button", { name: "Grant access…" }));

  expect(api.requestPermission).toHaveBeenCalledExactlyOnceWith("microphone");
  expect(api.openPermissionSettings).not.toHaveBeenCalled();
  expect(within(microphone).queryByRole("button", { name: "Open System Settings" })).not.toBeInTheDocument();
});

test("a microphone that was refused can only be turned on in System Settings, and the row says where", async () => {
  const user = userEvent.setup();
  status("denied", "granted");
  render(Permissions);
  const microphone = await screen.findByRole("listitem", { name: "Microphone" });

  // The system doesn't show its prompt a second time, so a button for it would do nothing.
  expect(within(microphone).queryByRole("button", { name: "Grant access…" })).not.toBeInTheDocument();
  expect(microphone).toHaveTextContent(
    "Dictate doesn't have Microphone access — turn it on in System Settings → Privacy & Security.",
  );
  await user.click(within(microphone).getByRole("button", { name: "Open System Settings" }));
  expect(api.openPermissionSettings).toHaveBeenCalledExactlyOnceWith("microphone");
});

test("accessibility that is missing says what it is for and has both buttons", async () => {
  const user = userEvent.setup();
  status("granted", "denied");
  render(Permissions);
  const accessibility = await screen.findByRole("listitem", { name: "Accessibility" });

  expect(accessibility).toHaveTextContent("Writing into text fields needs Accessibility permission");
  await user.click(within(accessibility).getByRole("button", { name: "Grant access…" }));
  expect(api.requestPermission).toHaveBeenCalledExactlyOnceWith("accessibility");
  await user.click(within(accessibility).getByRole("button", { name: "Open System Settings" }));
  expect(api.openPermissionSettings).toHaveBeenCalledExactlyOnceWith("accessibility");
});

test("a permission that is granted has no buttons, and the note about a rebuilt app is shown", async () => {
  status("granted", "granted");
  render(Permissions);
  await screen.findByRole("heading", { name: "Permissions" });

  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  expect(screen.getByText(/rebuilt or updated/)).toHaveTextContent(
    "An app that is rebuilt or updated without a signature may need Accessibility granted again: in System Settings, remove Dictate from the list and add it again.",
  );
});

test("a button that fails shows why, and the section goes on", async () => {
  const user = userEvent.setup();
  status("denied", "granted");
  vi.mocked(api.openPermissionSettings).mockRejectedValue("Could not open System Settings");
  render(Permissions);

  await user.click(await screen.findByRole("button", { name: "Open System Settings" }));

  expect(await screen.findByRole("alert")).toHaveTextContent("Could not open System Settings");
});

test("a status that can't be read leaves the section as it was", async () => {
  vi.useFakeTimers();
  status("granted", "denied");
  render(Permissions);
  await vi.advanceTimersByTimeAsync(0);
  expect(within(row("Accessibility")).getByText("Not allowed")).toBeInTheDocument();

  vi.mocked(api.permissionsStatus).mockRejectedValue("the system did not answer");
  await vi.advanceTimersByTimeAsync(1000);

  expect(within(row("Accessibility")).getByText("Not allowed")).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("speaks Vietnamese when the interface does", async () => {
  setLanguage("vi");
  status("not_determined", "denied");
  render(Permissions);

  expect(await screen.findByRole("heading", { name: "Quyền truy cập" })).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Cấp quyền…" })).toHaveLength(2);
  expect(screen.getAllByRole("button", { name: "Mở System Settings" })).toHaveLength(1);
});

test("the_screen_row_is_shown_on_macos_only: Screen Recording is a row of the section, and there is none where the system asks for nothing", async () => {
  status("granted", "granted", "granted");
  const { unmount } = render(Permissions);

  const screenRow = await screen.findByRole("listitem", { name: "Screen Recording" });
  expect(within(screenRow).getByText("Allowed")).toBeInTheDocument();
  expect(screenRow).toHaveTextContent("Needed to record the screen and the sound of the system.");
  expect(within(screenRow).queryByRole("button")).not.toBeInTheDocument();
  unmount();

  vi.mocked(api.permissionsStatus).mockClear();
  vi.mocked(api.permissionsStatus).mockResolvedValue(NOT_MAC);
  render(Permissions);
  await vi.waitFor(() => expect(api.permissionsStatus).toHaveBeenCalled());
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByRole("listitem", { name: "Screen Recording" })).not.toBeInTheDocument();
});

test("a Screen Recording that is off has both buttons, which ask the system for the screen's permission", async () => {
  const user = userEvent.setup();
  // macOS can't say that it was never asked: until it is on, the screen is "not allowed".
  status("granted", "granted", "denied");
  render(Permissions);
  const screenRow = await screen.findByRole("listitem", { name: "Screen Recording" });

  expect(within(screenRow).getByText("Not allowed")).toBeInTheDocument();
  await user.click(within(screenRow).getByRole("button", { name: "Grant access…" }));
  expect(api.requestPermission).toHaveBeenCalledExactlyOnceWith("screen");
  await user.click(within(screenRow).getByRole("button", { name: "Open System Settings" }));
  expect(api.openPermissionSettings).toHaveBeenCalledExactlyOnceWith("screen");
});

test("Screen Recording has words in Vietnamese too", async () => {
  setLanguage("vi");
  status("granted", "granted", "denied");
  render(Permissions);

  const screenRow = await screen.findByRole("listitem", { name: "Screen Recording" });
  expect(screenRow).toHaveTextContent("Cần để quay màn hình và âm thanh hệ thống.");
});
