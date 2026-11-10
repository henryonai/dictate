import { render } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { PlayerController } from "../lib/player.svelte";
import { fakeMedia } from "../test-helpers";
import Player from "./Player.svelte";

const ID = "2026-03-02 10-00-00";
const OLD = fakeMedia({ url: "http://127.0.0.1:41000/tok-a/1" });
const NEW = fakeMedia({ url: "http://127.0.0.1:52000/tok-b/1" });

beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
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
