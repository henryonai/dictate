// The names that speakers are given in the rename sheet, cleaned as the Rust side cleans them
// (`dictate_core::meetings::speakers::clean`): the sheet says what will happen before it is asked for.

/** The longest name a transcript line still reads as a speaker. */
export const MAX_NAME_LENGTH = 24;

const graphemes =
  typeof Intl !== "undefined" && "Segmenter" in Intl ? new Intl.Segmenter(undefined, { granularity: "grapheme" }) : null;

/** The characters of the text as a person counts them: a letter and the marks put on it are one. */
function split(text: string): string[] {
  return graphemes ? Array.from(graphemes.segment(text), (part) => part.segment) : Array.from(text);
}

/** The words of the text, split on whitespace. */
function wordsOf(text: string): string[] {
  return text.split(/\s+/).filter(Boolean);
}

/** The text with its spaces made single, and nothing else changed. */
function spaced(name: string): string {
  return wordsOf(name).join(" ");
}

/**
 * A name as it can stand before ": " in a transcript line: on one line, without ":" "." or "*", at most 24 characters, cut between
 * words (a first word that is longer is cut itself). Null when nothing is left.
 */
export function cleanSpeakerName(name: string): string | null {
  let cleaned = "";
  for (const word of wordsOf(name.replace(/[:.*]/g, ""))) {
    const next = cleaned === "" ? split(word).slice(0, MAX_NAME_LENGTH).join("") : `${cleaned} ${word}`;
    if (split(next).length > MAX_NAME_LENGTH) break;
    cleaned = next;
  }
  return cleaned === "" ? null : cleaned;
}

/** Speakers that end up with one name. */
export interface Merge {
  /** The names they have now, in the order of the speakers. */
  from: string[];
  /** The name they will share. */
  into: string;
}

/** What the names that were typed will do. */
export interface Renaming {
  /** Old name to new, for the speakers whose name changes: what the Rust side is given. */
  names: Record<string, string>;
  merges: Merge[];
  /** Something typed is not what will stay (it was too long, or had a ":" "." or "*"): a hint says so. */
  trimmed: boolean;
}

/** The names that `typed` (by the speaker's name now; empty is no change) give to the speakers, in the order they talk. */
export function planRenames(speakers: string[], typed: Record<string, string>): Renaming {
  const names: Record<string, string> = {};
  let trimmed = false;
  for (const speaker of speakers) {
    const text = typed[speaker] ?? "";
    if (text.trim() === "") continue;
    const cleaned = cleanSpeakerName(text);
    if (cleaned !== spaced(text)) trimmed = true;
    if (cleaned !== null && cleaned !== speaker) names[speaker] = cleaned;
  }
  const sharing = new Map<string, string[]>();
  for (const speaker of speakers) {
    const name = names[speaker] ?? speaker;
    sharing.set(name, [...(sharing.get(name) ?? []), speaker]);
  }
  const merges = [...sharing].filter(([, from]) => from.length > 1).map(([into, from]) => ({ from, into }));
  return { names, merges, trimmed };
}
