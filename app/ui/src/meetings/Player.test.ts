import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { declarationsOf, parseCss, styleOf } from "../css-rules";
import { setLanguage } from "../lib/i18n";
import { PlayerController } from "../lib/player.svelte";
import { fakeMedia } from "../test-helpers";
import Player from "./Player.svelte";

const ID = "2026-03-02 10-00-00";
const OLD = fakeMedia({ url: "http://127.0.0.1:41000/tok-a/1" });
const NEW = fakeMedia({ url: "http://127.0.0.1:52000/tok-b/1" });

beforeEach(() => {
  localStorage.clear();
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  setLanguage("en");
});

test("the element is given its address once, and once more for each address that is swapped in", async () => {
  const sources = vi.spyOn(HTMLMediaElement.prototype, "src", "set");
  const player = new PlayerController(async () => OLD);
  player.bind(ID, OLD);
  render(Player, { player });
  await tick();

  expect(sources.mock.calls.map(([address]) => address)).toEqual([OLD.url]);

  // A swap changes `player.url`, which the element's own effect must not depend on: it would be attached again, and loaded again.
  player.bind(ID, NEW);
  await tick();
  expect(sources.mock.calls.map(([address]) => address)).toEqual([OLD.url, NEW.url]);

  player.bind(ID, fakeMedia({ url: "http://127.0.0.1:63000/tok-c/1" }));
  await tick();
  expect(sources.mock.calls.map(([address]) => address)).toEqual([OLD.url, NEW.url, "http://127.0.0.1:63000/tok-c/1"]);
});

test("the element is the same one across swaps, and is gone when the player lets go of the file", async () => {
  const player = new PlayerController(async () => OLD);
  player.bind(ID, OLD);
  render(Player, { player });
  await tick();
  const element = document.querySelector("video");
  expect(element).not.toBeNull();

  player.bind(ID, NEW);
  await tick();
  expect(document.querySelector("video")).toBe(element);

  player.release();
  await tick();
  expect(document.querySelector("video")).toBeNull();
});

// ---- A player sized to the recording

const SIZE_KEY = "loah.player.size";
const AUDIO = fakeMedia({ url: "http://127.0.0.1:41000/tok-a/audio", video: false });
const source = styleOf(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "Player.svelte"), "utf8"));
const styles = parseCss(source);

/** A player on a recording with `media`, shown. */
async function show(media = OLD) {
  const player = new PlayerController(async () => media);
  player.bind(ID, media);
  const view = render(Player, { player });
  await tick();
  return { player, view };
}

/** The element of the player, which has loaded its metadata: the size of its picture (0 for none) and how long it plays. */
async function loaded(element: HTMLMediaElement, { width = 1280, duration = 600 } = {}) {
  Object.defineProperty(element, "videoWidth", { value: width, configurable: true });
  Object.defineProperty(element, "duration", { value: duration, configurable: true });
  element.dispatchEvent(new Event("loadedmetadata"));
  await tick();
}

/** The element is playing, as it says with its events. */
async function playing(element: HTMLMediaElement) {
  Object.defineProperty(element, "paused", { value: false, configurable: true });
  element.dispatchEvent(new Event("play"));
  await tick();
}

const video = () => document.querySelector("video") as HTMLVideoElement;
const bar = () => document.querySelector(".bar") as HTMLElement | null;
/** What the bar says about the time: where it is and how long it is. */
const time = () => (bar()?.querySelector(".time") as HTMLElement).textContent?.replace(/\s+/g, " ").trim();
/** The picture is put out of sight (and still plays), as a style that is not `display: none`. */
const away = (element: Element) => element.classList.contains("away");

test("an_audio_recording_gets_the_slim_bar: no video box, and a bar of play, position, time and speed", async () => {
  await show(AUDIO);
  const audio = document.querySelector("audio") as HTMLAudioElement;
  await loaded(audio, { duration: 754 });

  expect(document.querySelector("video")).toBeNull();
  // The bar draws the controls: the element has none of its own.
  expect(audio).not.toHaveAttribute("controls");
  const slim = within(bar() as HTMLElement);
  expect(slim.getByRole("button", { name: "Play" })).toBeInTheDocument();
  expect(slim.getByRole("slider", { name: "Position" })).toHaveAttribute("max", "754");
  expect(time()).toBe("00:00 / 12:34");
  // The speed is one button that shows the speed it has, and goes to the next one.
  const speed = slim.getByRole("button", { name: "Playback speed 1×" });
  expect(speed.textContent?.trim()).toBe("1×");
  expect(slim.getAllByRole("button")).toHaveLength(2);
  // Nothing that is for a picture.
  for (const name of ["Enlarge", "Hide video", "Show video"]) expect(screen.queryByRole("button", { name })).toBeNull();
  // It is slim: 48 px or less.
  expect(Number.parseInt(declarationsOf(styles, ".bar").height, 10)).toBeLessThanOrEqual(48);
});

test("an_audio_recording_gets_the_slim_bar: it plays and pauses, goes to a position, and changes the speed", async () => {
  const user = userEvent.setup();
  const { player } = await show(AUDIO);
  const audio = document.querySelector("audio") as HTMLAudioElement;
  await loaded(audio, { duration: 600 });

  await user.click(screen.getByRole("button", { name: "Play" }));
  expect(audio.play).toHaveBeenCalledTimes(1);
  await playing(audio);
  // It plays now: the button pauses.
  await user.click(screen.getByRole("button", { name: "Pause" }));
  expect(audio.pause).toHaveBeenCalledTimes(1);

  const position = screen.getByRole("slider", { name: "Position" }) as HTMLInputElement;
  position.value = "95";
  position.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
  expect(audio.currentTime).toBe(95);
  expect(player.currentTime).toBe(95);
  expect(time()).toBe("01:35 / 10:00");
  expect(position).toHaveAttribute("aria-valuetext", "01:35 / 10:00");

  // 1×, 1.5×, 2× and round again.
  const speed = (rate: string) => screen.getByRole("button", { name: `Playback speed ${rate}×` });
  await user.click(speed("1"));
  expect(audio.playbackRate).toBe(1.5);
  expect(speed("1.5").textContent?.trim()).toBe("1.5×");
  await user.click(speed("1.5"));
  expect(audio.playbackRate).toBe(2);
  // The speed is the one that is chosen when the next address has loaded: a load puts the element back to 1.
  audio.playbackRate = 1;
  await loaded(audio);
  expect(audio.playbackRate).toBe(2);
  await user.click(speed("2"));
  expect(audio.playbackRate).toBe(1);
  expect(player.rate).toBe(1);
});

test("an_audio_recording_gets_the_slim_bar: a speed that the native menu chose is the one the bar shows, and goes on from there", async () => {
  const user = userEvent.setup();
  await show(AUDIO);
  const audio = document.querySelector("audio") as HTMLAudioElement;
  await loaded(audio);

  audio.playbackRate = 1.25;
  audio.dispatchEvent(new Event("ratechange"));
  await tick();
  expect(screen.getByRole("button", { name: "Playback speed 1.25×" }).textContent?.trim()).toBe("1.25×");

  // The next one up, of the three of the bar.
  await user.click(screen.getByRole("button", { name: "Playback speed 1.25×" }));
  expect(audio.playbackRate).toBe(1.5);
});

test("a_video_with_a_picture_keeps_the_video_and_its_two_buttons", async () => {
  await show(OLD);
  await loaded(video(), { width: 1280 });

  expect(away(video())).toBe(false);
  expect(video()).not.toHaveAttribute("hidden");
  expect(video()).toHaveAttribute("controls");
  expect(bar()).toBeNull();
  expect(screen.getByRole("button", { name: "Enlarge" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Hide video" })).toBeInTheDocument();
  // About a third of the column, and no more than 360 px; Enlarge makes it up to 70 % of the window, 720 px at most.
  expect(declarationsOf(styles, "video").height).toBe("min(33vh, 360px)");
  expect(source).toMatch(/video\.large \{\s*height: min\(70vh, 720px\);/);
});

test("enlarging_does_not_squeeze_the_transcript: the large picture is capped by the height of its column, and the column says its height", () => {
  // `cqh` is the height of the column (`.left` is a container of its size): what the picture leaves for the controls and the transcript.
  // A web view that does not know `cqh` keeps the rule that has none; the one that knows it is capped.
  expect(declarationsOf(styles, "video.large").height).toBe("min(70vh, 720px)");
  const capped = styles.find((rule) => rule.supports === "(height: 1cqh)" && rule.selectors.includes("video.large"));
  const large = capped?.declarations.height ?? "";
  expect(large).toBe("min(70vh, 720px, calc(100cqh - 200px))");
  const detail = parseCss(styleOf(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "Detail.svelte"), "utf8")));
  expect(declarationsOf(detail, ".left")["container-type"]).toBe("size");
  // The picture never takes more than the column less 200 px, whatever the window.
  const left = Number(/calc\(100cqh - (\d+)px\)/.exec(large)?.[1]);
  expect(left).toBeGreaterThanOrEqual(200);
});

test("a_video_with_no_picture_falls_back_to_the_slim_bar: an audio file with a video extension has no black box", async () => {
  const { player } = await show(OLD);
  // Until it has loaded the element is a video, as the extension says.
  expect(screen.getByRole("button", { name: "Hide video" })).toBeInTheDocument();

  await loaded(video(), { width: 0 });

  expect(away(video())).toBe(true);
  expect(video()).not.toHaveAttribute("controls");
  expect(within(bar() as HTMLElement).getByRole("button", { name: "Play" })).toBeInTheDocument();
  expect(time()).toBe("00:00 / 10:00");
  // There is no picture to enlarge, hide or show, and what was chosen for the pictures is left as it was.
  for (const name of ["Enlarge", "Hide video", "Show video"]) expect(screen.queryByRole("button", { name })).toBeNull();
  expect(localStorage.getItem(SIZE_KEY)).toBeNull();
  expect(player.size).toBe("small");

  // The next recording is looked at again: this one has a picture.
  player.bind("another", fakeMedia({ url: "http://127.0.0.1:41000/tok-a/2" }));
  await tick();
  await loaded(video(), { width: 640 });
  expect(away(video())).toBe(false);
  expect(bar()).toBeNull();
  expect(screen.getByRole("button", { name: "Hide video" })).toBeInTheDocument();
});

test("hide_video_keeps_playing: the video goes away, the sound does not stop, and the slim bar takes over", async () => {
  const user = userEvent.setup();
  const sources = vi.spyOn(HTMLMediaElement.prototype, "src", "set");
  await show(OLD);
  const element = video();
  await loaded(element);
  await playing(element);
  expect(sources).toHaveBeenCalledTimes(1);

  await user.click(screen.getByRole("button", { name: "Hide video" }));

  // The same element, not paused, not loaded again: only out of sight.
  expect(video()).toBe(element);
  expect(away(element)).toBe(true);
  expect(element).not.toHaveAttribute("hidden");
  expect(element.pause).not.toHaveBeenCalled();
  expect(element.load).not.toHaveBeenCalled();
  expect(element.paused).toBe(false);
  expect(sources).toHaveBeenCalledTimes(1);
  // The bar shows that it plays, and offers the video again.
  expect(within(bar() as HTMLElement).getByRole("button", { name: "Pause" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Hide video" })).toBeNull();

  await user.click(screen.getByRole("button", { name: "Show video" }));
  expect(video()).toBe(element);
  expect(away(element)).toBe(false);
  expect(bar()).toBeNull();
  expect(element.pause).not.toHaveBeenCalled();
});

test("hide_video_keeps_playing: the picture is out of sight with a style that leaves it in the page, not with display none or the hidden attribute", () => {
  // A video that is not drawn at all may be stopped by an engine; one that is a pixel and see-through is still a video that plays.
  const out = declarationsOf(styles, "video.away");
  expect(out.position).toBe("absolute");
  expect(out.width).toBe("1px");
  expect(out.height).toBe("1px");
  expect(out.opacity).toBe("0");
  expect(out["pointer-events"]).toBe("none");
  expect(out).not.toHaveProperty("display");
  expect(declarationsOf(styles, ".player").position).toBe("relative");
  // No rule of the video takes it out of the layout (the total of the time, which is not the video, goes with a container rule).
  const ofTheVideo = styles.filter((rule) => rule.selectors.some((selector) => selector.startsWith("video")));
  expect(ofTheVideo.length).toBeGreaterThan(0);
  for (const rule of ofTheVideo) expect(rule.declarations.display).not.toBe("none");
});

test("hide_video_keeps_playing: the keys go with the buttons, to the one that takes the place of the one that was pressed", async () => {
  const user = userEvent.setup();
  await show(OLD);
  await loaded(video());

  await user.click(screen.getByRole("button", { name: "Hide video" }));
  await tick();
  expect(screen.getByRole("button", { name: "Show video" })).toHaveFocus();

  await user.click(screen.getByRole("button", { name: "Show video" }));
  await tick();
  expect(screen.getByRole("button", { name: "Hide video" })).toHaveFocus();

  // With the keys: Enter on the focused button does the same.
  await user.keyboard("{Enter}");
  await tick();
  expect(screen.getByRole("button", { name: "Show video" })).toHaveFocus();
});

test("the_size_choice_is_remembered: Enlarge, Hide video and Show video are kept for the next recording and the next launch", async () => {
  const user = userEvent.setup();
  const first = await show(OLD);
  await loaded(video());
  expect(localStorage.getItem(SIZE_KEY)).toBeNull();

  await user.click(screen.getByRole("button", { name: "Enlarge" }));
  expect(localStorage.getItem(SIZE_KEY)).toBe("large");
  expect(video()).toHaveClass("large");
  // The button that enlarged it makes it small again.
  expect(screen.queryByRole("button", { name: "Enlarge" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Shrink" }));
  expect(localStorage.getItem(SIZE_KEY)).toBe("small");
  expect(video()).not.toHaveClass("large");
  await user.click(screen.getByRole("button", { name: "Enlarge" }));

  // Another recording in the same window: still large.
  first.player.bind("another", fakeMedia({ url: "http://127.0.0.1:41000/tok-a/2" }));
  await tick();
  await loaded(video());
  expect(video()).toHaveClass("large");

  // Another launch: a new player reads it.
  first.view.unmount();
  const second = await show(OLD);
  expect(second.player.size).toBe("large");
  expect(video()).toHaveClass("large");

  await loaded(video());
  await user.click(screen.getByRole("button", { name: "Hide video" }));
  expect(localStorage.getItem(SIZE_KEY)).toBe("hidden");
  second.view.unmount();

  // Hidden is remembered too: the next one opens on the slim bar, with the picture a click away, as large as it was.
  const third = await show(OLD);
  expect(third.player.size).toBe("hidden");
  expect(away(video())).toBe(true);
  await user.click(screen.getByRole("button", { name: "Show video" }));
  expect(localStorage.getItem(SIZE_KEY)).toBe("small");
  expect(video()).not.toHaveClass("large");
});

test("the_size_choice_is_remembered: a value that is not a size is ignored, and a store that refuses does not break the player", async () => {
  const user = userEvent.setup();
  localStorage.setItem(SIZE_KEY, "gigantic");
  const unknown = await show(OLD);
  expect(unknown.player.size).toBe("small");
  unknown.view.unmount();

  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
    throw new DOMException("denied", "SecurityError");
  });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new DOMException("denied", "QuotaExceededError");
  });
  const { player } = await show(OLD);
  await loaded(video());

  expect(player.size).toBe("small");
  await user.click(screen.getByRole("button", { name: "Enlarge" }));
  // It holds for this window, though it can't be kept.
  expect(player.size).toBe("large");
});

test("the player says its words in Vietnamese", async () => {
  setLanguage("vi");
  await show(AUDIO);
  await loaded(document.querySelector("audio") as HTMLAudioElement);

  const slim = within(bar() as HTMLElement);
  expect(slim.getByRole("button", { name: "Phát" })).toBeInTheDocument();
  expect(slim.getByRole("slider", { name: "Vị trí" })).toBeInTheDocument();
  expect(slim.getByRole("button", { name: "Tốc độ phát 1×" })).toBeInTheDocument();
});

test("the video buttons say their words in Vietnamese", async () => {
  const user = userEvent.setup();
  setLanguage("vi");
  await show(OLD);
  await loaded(video());

  await user.click(screen.getByRole("button", { name: "Phóng to" }));
  expect(screen.getByRole("button", { name: "Thu nhỏ" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Ẩn video" }));
  expect(screen.getByRole("button", { name: "Hiện video" })).toBeInTheDocument();
});
