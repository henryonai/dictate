// A button that turns into Cancel in the same spot (Download, Retry, Transcribe) catches the second click of a
// double click on it: the job would start and be cancelled at once. A Cancel this soon after the start is taken
// for that second click and ignored; the person who meant it clicks again.
export const DOUBLE_CLICK_MS = 500;

/**
 * The moment a job starts, for `tooSoonToCancel`. It is read from the clock that only goes forward: the wall clock
 * can be set back (a time sync, a person) and would make a young job of an old one, and block its Cancel.
 */
export function startedNow(): number {
  return performance.now();
}

/** Whether something that started at `startedAt` (from `startedNow()`) is too young to be cancelled. */
export function tooSoonToCancel(startedAt: number | undefined): boolean {
  return startedAt !== undefined && performance.now() - startedAt < DOUBLE_CLICK_MS;
}
