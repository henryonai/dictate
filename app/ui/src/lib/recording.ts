// What the windows of the recording have in common: the state of a recording that is not going on, and the shortcut that starts
// and stops one (which is the same everywhere in the app and not set by the person).
import { onMac } from "../panel/state";
import type { RecordingView } from "./api";

/** What the recorder reports when nothing is recorded. */
export const IDLE_RECORDING: RecordingView = {
  phase: "idle",
  elapsedMs: 0,
  levels: { mic: null, system: null },
  mode: null,
  meetingId: null,
  notes: [],
};

/** The shortcut of the recording as the system writes it: "⌃⌥R" on a Mac, "Ctrl+Alt+R" elsewhere. */
export function recordHotkey(): string {
  return onMac() ? "⌃⌥R" : "Ctrl+Alt+R";
}
