// The texts that the window copies: the transcript as the file has it, and the action items as a checklist.
import type { ActionView, LineView } from "./api";

/** The transcript as `transcript.txt` has it: `[stamp] Speaker: text` paragraphs with a blank line between them. */
export function transcriptText(lines: LineView[]): string {
  return lines
    .map((line) => `${line.stamp ? `[${line.stamp}] ` : ""}${line.speaker ? `${line.speaker}: ` : ""}${line.text}`)
    .join("\n\n");
}

/** The action items as a Markdown checklist: `- [x] **Owner** — text` when ticked, `- [ ] text` when not (an owner is named either way). */
export function checklistText(actions: ActionView[]): string {
  return actions
    .map((action) => `- [${action.done ? "x" : " "}] ${action.owner ? `**${action.owner}** — ` : ""}${action.text}`)
    .join("\n");
}
