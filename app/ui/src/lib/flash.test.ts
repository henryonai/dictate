import { afterEach, expect, test, vi } from "vitest";
import { Flash } from "./flash.svelte";

afterEach(() => vi.useRealTimers());

test("is on for a moment after it is shown, then off", () => {
  vi.useFakeTimers();
  const flash = new Flash();
  expect(flash.on).toBe(false);

  flash.show();
  expect(flash.on).toBe(true);

  vi.advanceTimersByTime(1999);
  expect(flash.on).toBe(true);
  vi.advanceTimersByTime(1);
  expect(flash.on).toBe(false);
});

test("a second show starts the moment again", () => {
  vi.useFakeTimers();
  const flash = new Flash();

  flash.show();
  vi.advanceTimersByTime(1500);
  flash.show();
  vi.advanceTimersByTime(1500);

  expect(flash.on).toBe(true);
  vi.advanceTimersByTime(500);
  expect(flash.on).toBe(false);
});

test("leaves no timer behind when it is cleared", () => {
  vi.useFakeTimers();
  const flash = new Flash();
  flash.show();

  flash.clear();

  expect(vi.getTimerCount()).toBe(0);
});
