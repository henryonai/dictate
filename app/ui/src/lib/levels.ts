// The meters of the recording bar: how many bars a level lights, and what a level says about a track.

/** How many bars a meter has. */
export const BAR_COUNT = 6;

/** A track hears something when its level is above this. */
export const HEARING_LEVEL = 0.05;

/** Which bars are lit at `level` (0 to 1; null is a track that is off): bar `i` is lit when `i / 6` is below the level. */
export function barsLit(level: number | null): boolean[] {
  const value = level === null || Number.isNaN(level) ? 0 : level;
  return Array.from({ length: BAR_COUNT }, (_, i) => i / BAR_COUNT < value);
}

/** The height of bar `i` in points: 6, then 2 more for each bar. */
export function barHeight(i: number): number {
  return 6 + i * 2;
}

/** Whether a track picks up sound at `level`: what the tooltip of its meter says. */
export function hearsSound(level: number | null): boolean {
  return level !== null && level > HEARING_LEVEL;
}
