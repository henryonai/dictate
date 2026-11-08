// A button that turns into Cancel in the same spot (Download, Retry, Transcribe) catches the second click of a
// double click on it: the job would start and be cancelled at once. A Cancel this soon after the start is taken
// for that second click and ignored; the person who meant it clicks again.
export const DOUBLE_CLICK_MS = 500;

/** Whether something that started at `startedAt` (`Date.now()` then) is too young to be cancelled. */
export function tooSoonToCancel(startedAt: number | undefined): boolean {
  return startedAt !== undefined && Date.now() - startedAt < DOUBLE_CLICK_MS;
}
