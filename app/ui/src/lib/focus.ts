// A sheet or dialog over the window keeps the focus: Tab goes round what is in it.

/** What can take the focus in a sheet. */
const FOCUSABLE = "button:not(:disabled), input:not(:disabled), a[href]";

/**
 * Call it on the Tab key of a window that has a sheet open: at the last control Tab goes to the first, and Shift+Tab at the first
 * goes to the last, so that the focus doesn't reach the page behind.
 */
export function trapTab(event: KeyboardEvent, sheet: HTMLElement | undefined): void {
  if (event.key !== "Tab" || !sheet) return;
  const controls = [...sheet.querySelectorAll<HTMLElement>(FOCUSABLE)];
  if (controls.length === 0) return;
  const first = controls[0];
  const last = controls[controls.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
