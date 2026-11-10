import { expect, test } from "vitest";
import { cleanSpeakerName, MAX_NAME_LENGTH, planRenames } from "./speaker-names";

test("a name is at most 24 characters, as the Rust side keeps it", () => {
  expect(MAX_NAME_LENGTH).toBe(24);
});

test("a name has no colon, dot or asterisk, and its spaces are single ones", () => {
  expect(cleanSpeakerName("  Dr. An: *the boss*  ")).toBe("Dr An the boss");
  expect(cleanSpeakerName("chị   Lan")).toBe("chị Lan");
});

test("a name with nothing left is none", () => {
  expect(cleanSpeakerName("")).toBeNull();
  expect(cleanSpeakerName("  ")).toBeNull();
  expect(cleanSpeakerName(":.*")).toBeNull();
});

test("a long name is cut between words, and a first word that is too long is cut itself", () => {
  expect(cleanSpeakerName("Nguyễn Văn Anh Tuấn")).toBe("Nguyễn Văn Anh Tuấn");
  // 11 + 1 + 12 = 24 fits exactly; a word more would make it 26, and one letter more in the second word, 25.
  expect(cleanSpeakerName("aaaaaaaaaaa bbbbbbbbbbbb")).toBe("aaaaaaaaaaa bbbbbbbbbbbb");
  expect(cleanSpeakerName("aaaaaaaaaaa bbbbbbbbbbbb c")).toBe("aaaaaaaaaaa bbbbbbbbbbbb");
  expect(cleanSpeakerName("aaaaaaaaaaa bbbbbbbbbbbbb")).toBe("aaaaaaaaaaa");
  expect(cleanSpeakerName("a".repeat(30))).toBe("a".repeat(24));
  expect(cleanSpeakerName("a".repeat(30) + " b")).toBe("a".repeat(24));
});

test("characters are counted as a person sees them: a letter with its marks apart is one", () => {
  // "ễ" as e + circumflex + tilde: 3 code points, one character.
  const name = "ễ".repeat(24);
  expect(cleanSpeakerName(name)).toBe(name);
  expect(cleanSpeakerName(name + "e")).toBe(name);
});

test("planRenames_gives_the_names_that_change_and_leaves_the_rest", () => {
  const plan = planRenames(["Speaker 1", "Speaker 2", "Speaker 3"], {
    "Speaker 1": "  An ",
    "Speaker 2": "",
    "Speaker 3": "Speaker 3",
  });

  expect(plan.names).toEqual({ "Speaker 1": "An" });
  expect(plan.merges).toEqual([]);
  expect(plan.trimmed).toBe(false);
});

test("planRenames_sends_the_name_as_it_is_cleaned", () => {
  const plan = planRenames(["Speaker 1"], { "Speaker 1": "Dr. An" });

  expect(plan.names).toEqual({ "Speaker 1": "Dr An" });
  expect(plan.trimmed).toBe(true);
});

test("planRenames_a_name_that_is_all_marks_is_no_change", () => {
  const plan = planRenames(["Speaker 1"], { "Speaker 1": ":" });

  expect(plan.names).toEqual({});
  // It was typed and it is not what stays: the hint says why nothing happens.
  expect(plan.trimmed).toBe(true);
});

test("planRenames_two_speakers_with_one_name_are_merged", () => {
  const plan = planRenames(["Speaker 1", "Speaker 2", "Speaker 3"], {
    "Speaker 1": "An",
    "Speaker 2": "An ",
  });

  expect(plan.merges).toEqual([{ from: ["Speaker 1", "Speaker 2"], into: "An" }]);
});

test("planRenames_a_speaker_given_the_name_of_one_that_stays_is_merged_with_it", () => {
  const plan = planRenames(["Speaker 1", "Speaker 2"], { "Speaker 1": "Speaker 2" });

  expect(plan.merges).toEqual([{ from: ["Speaker 1", "Speaker 2"], into: "Speaker 2" }]);
});

test("planRenames_a_swap_is_not_a_merge", () => {
  const plan = planRenames(["Speaker 1", "Speaker 2"], { "Speaker 1": "Speaker 2", "Speaker 2": "Speaker 1" });

  expect(plan.names).toEqual({ "Speaker 1": "Speaker 2", "Speaker 2": "Speaker 1" });
  expect(plan.merges).toEqual([]);
});

test("planRenames_merges_come_in_the_order_of_the_speakers", () => {
  const plan = planRenames(["Bạn", "Speaker 2", "Speaker 3", "Speaker 4"], {
    "Speaker 2": "Lan",
    "Speaker 3": "Bạn",
    "Speaker 4": "Lan",
  });

  expect(plan.merges).toEqual([
    { from: ["Bạn", "Speaker 3"], into: "Bạn" },
    { from: ["Speaker 2", "Speaker 4"], into: "Lan" },
  ]);
});

test("planRenames_a_name_that_is_only_longer_than_24_is_said_to_be_cut", () => {
  const plan = planRenames(["Speaker 1"], { "Speaker 1": "a".repeat(30) });

  expect(plan.trimmed).toBe(true);
  expect(plan.names).toEqual({ "Speaker 1": "a".repeat(24) });
});
