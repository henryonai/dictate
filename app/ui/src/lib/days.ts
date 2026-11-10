// How the library tells time: the day groups of the list, and the clocks of a job and of a recording.
import type { MeetingRow } from "./api";
import { currentLanguage, t } from "./i18n";

export interface DayGroup {
  label: string;
  rows: MeetingRow[];
}

/** A key that is the same for every moment of one local day. `older` is the recordings that have no date. */
function dayKey(created: number): string {
  if (!created) return "older";
  const day = new Date(created * 1000);
  return `${day.getFullYear()}-${day.getMonth()}-${day.getDate()}`;
}

/** The day before `now`'s, on the calendar: not 24 hours ago, which is another day for part of the night. */
function dayBefore(now: Date): Date {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
}

function labelOf(key: string, created: number, now: Date): string {
  if (key === "older") return t("meetings.older");
  if (key === dayKey(now.getTime() / 1000)) return t("meetings.today");
  if (key === dayKey(dayBefore(now).getTime() / 1000)) return t("meetings.yesterday");
  return new Intl.DateTimeFormat(currentLanguage(), { day: "numeric", month: "long", year: "numeric" }).format(
    new Date(created * 1000),
  );
}

/**
 * The rows in groups by the day they were made, in the order they come: a run of rows of one day is one group (the list is
 * sorted by date, so a day is not split). A row with no date (0) is "Older".
 */
export function groupByDay(rows: MeetingRow[], now: Date): DayGroup[] {
  const groups: DayGroup[] = [];
  let lastKey: string | undefined;
  for (const row of rows) {
    const key = dayKey(row.created);
    if (key !== lastKey) {
      groups.push({ label: labelOf(key, row.created, now), rows: [] });
      lastKey = key;
    }
    groups[groups.length - 1].rows.push(row);
  }
  return groups;
}

/**
 * How many seconds a step has left, from how long it has run (`elapsedS`) and how far it is (`p`, 0 to 1): the pace so far
 * goes on. Nothing for the first 3 per cent, where a guess swings wildly, and nothing for a step that is done.
 */
export function timeLeft(elapsedS: number, p: number): number | null {
  if (!Number.isFinite(elapsedS) || !Number.isFinite(p) || p <= 0.03 || p >= 1) return null;
  return (Math.max(0, elapsedS) * (1 - p)) / p;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** A time as the transcript writes its stamps: `mm:ss`, and `h:mm:ss` from an hour on. Seconds are cut, not rounded. */
export function formatClock(seconds: number): string {
  const whole = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/** How long a recording is, in words that fit a line of the list: "45 s", "12 min", "1 h 5 min". */
export function formatDuration(seconds: number): string {
  const whole = Math.round(Math.max(0, seconds));
  if (whole < 60) return t("meetings.duration.s", { s: whole });
  const minutes = Math.round(whole / 60);
  if (minutes < 60) return t("meetings.duration.m", { m: minutes });
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m === 0 ? t("meetings.duration.h", { h }) : t("meetings.duration.hm", { h, m });
}
