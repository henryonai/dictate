import { afterEach, describe, expect, test } from "vitest";
import { fakeRow } from "../test-helpers";
import { formatClock, formatDuration, groupByDay, timeLeft } from "./days";
import { setLanguage } from "./i18n";

afterEach(() => setLanguage("en"));

/** Seconds since the epoch of a moment of the local day, as `created` has it. */
const at = (year: number, month: number, day: number, hour = 9, minute = 0) =>
  new Date(year, month - 1, day, hour, minute).getTime() / 1000;

/** Sunday 15 March 2026, noon. */
const now = new Date(2026, 2, 15, 12, 0, 0);

const row = (id: string, created: number) => fakeRow({ id, created });

describe("groupByDay", () => {
  test("groups_rows_by_day: today, yesterday, a long date for the days before, and older for a recording with no date", () => {
    const rows = [
      row("a", at(2026, 3, 15, 11)),
      row("b", at(2026, 3, 15, 8)),
      row("c", at(2026, 3, 14, 23, 50)),
      row("d", at(2026, 3, 3, 16)),
      row("e", at(2026, 3, 3, 9)),
      row("f", at(2025, 12, 31, 9)),
      row("g", 0),
    ];

    const groups = groupByDay(rows, now);

    expect(groups.map((group) => group.label)).toEqual([
      "Today",
      "Yesterday",
      "March 3, 2026",
      "December 31, 2025",
      "Older",
    ]);
    expect(groups.map((group) => group.rows.map((r) => r.id))).toEqual([["a", "b"], ["c"], ["d", "e"], ["f"], ["g"]]);
  });

  test("yesterday is the day before on the calendar, not 24 hours ago", () => {
    const justAfterMidnight = new Date(2026, 2, 15, 0, 30);
    const groups = groupByDay(
      [row("late", at(2026, 3, 14, 23, 50)), row("older", at(2026, 3, 13, 23, 50))],
      justAfterMidnight,
    );

    expect(groups.map((group) => group.label)).toEqual(["Yesterday", "March 13, 2026"]);
  });

  test("only a run of rows of the same day makes a group, so the order of the list is kept", () => {
    const groups = groupByDay(
      [row("a", at(2026, 3, 3)), row("b", at(2026, 3, 5)), row("c", at(2026, 3, 3, 8))],
      now,
    );

    expect(groups.map((group) => group.rows.map((r) => r.id))).toEqual([["a"], ["b"], ["c"]]);
  });

  test("a recording from the future gets its date, not a crash", () => {
    expect(groupByDay([row("a", at(2026, 3, 20))], now).map((group) => group.label)).toEqual(["March 20, 2026"]);
  });

  test("no rows, no groups", () => {
    expect(groupByDay([], now)).toEqual([]);
  });

  test("speaks Vietnamese when the language is", () => {
    setLanguage("vi");

    const groups = groupByDay([row("a", at(2026, 3, 15)), row("b", at(2026, 3, 14)), row("c", at(2026, 3, 3)), row("d", 0)], now);

    expect(groups.map((group) => group.label)).toEqual(["Hôm nay", "Hôm qua", expect.stringMatching(/3.*2026/), "Cũ hơn"]);
    expect(groups[2].label).toContain("tháng 3");
  });
});

describe("timeLeft", () => {
  test("time_left_only_between_3_and_100_percent: elapsed × (1 − p) / p, and nothing at either end", () => {
    // Under 3 per cent, or exactly 3, a guess would swing wildly.
    expect(timeLeft(30, 0)).toBeNull();
    expect(timeLeft(30, 0.02)).toBeNull();
    expect(timeLeft(30, 0.03)).toBeNull();
    expect(timeLeft(30, 0.031)).toBeCloseTo((30 * 0.969) / 0.031, 5);
    // Half done after a minute: a minute left.
    expect(timeLeft(60, 0.5)).toBe(60);
    expect(timeLeft(30, 0.25)).toBeCloseTo(90, 5);
    expect(timeLeft(30, 0.99)).toBeCloseTo(0.30303, 4);
    // Done, or more than done: there is no time left to tell.
    expect(timeLeft(30, 1)).toBeNull();
    expect(timeLeft(30, 1.2)).toBeNull();
  });

  test("no figure, a broken one, or a step that has not begun, gives none or nothing", () => {
    expect(timeLeft(30, Number.NaN)).toBeNull();
    expect(timeLeft(30, -0.5)).toBeNull();
    expect(timeLeft(Number.NaN, 0.5)).toBeNull();
    // A step that has not run for any time yet has no pace to tell the rest from.
    expect(timeLeft(0, 0.5)).toBe(0);
    expect(timeLeft(-5, 0.5)).toBe(0);
  });
});

describe("formatClock", () => {
  test("mm:ss, and h:mm:ss from an hour on, as the transcript's stamps are", () => {
    expect(formatClock(0)).toBe("00:00");
    expect(formatClock(65)).toBe("01:05");
    expect(formatClock(599.9)).toBe("09:59");
    expect(formatClock(3665)).toBe("1:01:05");
  });

  test("a time that has not begun, or is no number, is zero", () => {
    expect(formatClock(-3)).toBe("00:00");
    expect(formatClock(Number.NaN)).toBe("00:00");
  });
});

describe("formatDuration", () => {
  test("seconds under a minute, minutes under an hour, then hours and minutes", () => {
    expect(formatDuration(45)).toBe("45 s");
    expect(formatDuration(720)).toBe("12 min");
    expect(formatDuration(3900)).toBe("1 h 5 min");
    expect(formatDuration(7200)).toBe("2 h");
  });

  test("rounds to the nearest minute, and 59 minutes 40 seconds is not 59 minutes", () => {
    expect(formatDuration(3580)).toBe("1 h");
    expect(formatDuration(89)).toBe("1 min");
    expect(formatDuration(0.4)).toBe("0 s");
  });

  test("speaks Vietnamese when the language is", () => {
    setLanguage("vi");
    expect(formatDuration(45)).toBe("45 giây");
    expect(formatDuration(720)).toBe("12 phút");
    expect(formatDuration(3900)).toBe("1 giờ 5 phút");
    expect(formatDuration(7200)).toBe("2 giờ");
  });
});
