import { expect, test } from "vitest";
import type { ActionView, LineView } from "./api";
import { checklistText, transcriptText } from "./meeting-text";

test("copy_checklist_format: a box for each item, and the owner in bold when there is one", () => {
  const actions: ActionView[] = [
    { index: 0, owner: "An", text: "Send the budget by Friday", done: true },
    { index: 1, owner: null, text: "Book the room", done: false },
    { index: 2, owner: "Binh", text: "Review the draft", done: false },
  ];

  expect(checklistText(actions)).toBe(
    ["- [x] **An** — Send the budget by Friday", "- [ ] Book the room", "- [ ] **Binh** — Review the draft"].join("\n"),
  );
});

test("a checklist with no items is empty", () => {
  expect(checklistText([])).toBe("");
});

test("the transcript is copied in the form of transcript.txt: [stamp] Speaker: text, with a blank line between paragraphs", () => {
  const lines: LineView[] = [
    { start: 0, stamp: "00:00", speaker: "Speaker 1", text: "Let's start." },
    { start: 65, stamp: "01:05", speaker: null, text: "No name here." },
    { start: 0, stamp: "", speaker: "Speaker 2", text: "No stamp." },
    { start: 3725, stamp: "1:02:05", speaker: "Speaker 1", text: "Late." },
  ];

  expect(transcriptText(lines)).toBe(
    ["[00:00] Speaker 1: Let's start.", "[01:05] No name here.", "Speaker 2: No stamp.", "[1:02:05] Speaker 1: Late."].join("\n\n"),
  );
});
