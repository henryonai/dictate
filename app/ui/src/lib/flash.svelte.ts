// A thing that shows for a moment: the "Copied" of a Copy button.

/** How long it shows. */
export const FLASH_MS = 2000;

export class Flash {
  on = $state(false);
  private timer: ReturnType<typeof setTimeout> | undefined;

  /** Shows it, and takes it away after a moment; shown again, the moment starts again. */
  show(): void {
    this.on = true;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => (this.on = false), FLASH_MS);
  }

  /** For when the component goes away: no timer is left behind. */
  clear(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
  }
}
