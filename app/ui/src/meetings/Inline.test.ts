import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import Inline from "./Inline.svelte";

test("a time stamp in the text is a link that seeks to its second", async () => {
  const user = userEvent.setup();
  const onseek = vi.fn();
  render(Inline, { text: "See [01:05] and [1:02:05].", onseek });

  await user.click(screen.getByRole("link", { name: "[01:05]" }));
  expect(onseek).toHaveBeenLastCalledWith(65);

  await user.click(screen.getByRole("link", { name: "[1:02:05]" }));
  expect(onseek).toHaveBeenLastCalledWith(3725);
});

test("the link is reached and used with the keyboard too", async () => {
  const user = userEvent.setup();
  const onseek = vi.fn();
  render(Inline, { text: "At [00:42] it starts.", onseek });

  await user.tab();
  expect(screen.getByRole("link", { name: "[00:42]" })).toHaveFocus();
  await user.keyboard("{Enter}");

  expect(onseek).toHaveBeenCalledExactlyOnceWith(42);
});

test("a click on a stamp never follows the link (the page does not go to #), with a player or without", async () => {
  const withPlayer = render(Inline, { text: "At [00:42].", onseek: vi.fn() });
  // `fireEvent` says false when the click's default was prevented.
  expect(await fireEvent.click(screen.getByRole("link", { name: "[00:42]" }))).toBe(false);
  withPlayer.unmount();

  render(Inline, { text: "At [00:42]." });
  expect(await fireEvent.click(screen.getByText("[00:42]"))).toBe(false);
});

test("bold, lists and line breaks are shown, and the text is kept as it is written", () => {
  const { container } = render(Inline, { text: "- one **two**\n  - three" });

  expect(container.querySelector("strong")?.textContent).toBe("two");
  expect(container.textContent).toBe("•  one two\n      ◦  three");
});

test("a tag, a script or an address in the text shows as the text it is, and nothing runs", () => {
  const hostile = '<img src=x onerror="document.body.dataset.hacked=1"> <script>document.body.dataset.hacked=1</script> [x](javascript:alert(1))';
  const { container } = render(Inline, { text: hostile, onseek: vi.fn() });

  expect(container.textContent).toBe(hostile);
  expect(container.querySelector("img, script, a")).toBeNull();
  expect(document.body.dataset.hacked).toBeUndefined();
});

test("a link that has no player to seek is shown as plain text and does nothing", async () => {
  const user = userEvent.setup();
  render(Inline, { text: "At [00:42]." });
  const stamp = screen.getByText("[00:42]");

  expect(stamp).toHaveAttribute("data-inert");
  await user.click(stamp);
  expect(window.location.hash).toBe("");
});

test("the text that is shown changes with the text that is given", async () => {
  const view = render(Inline, { text: "first" });
  await view.rerender({ text: "**second**" });
  expect(view.container.querySelector("strong")?.textContent).toBe("second");
});
