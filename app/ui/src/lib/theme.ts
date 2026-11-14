// The theme of a window: the colors of tokens.css that the page draws with. `data-theme="light"` or `"dark"` on <html> forces one;
// with no attribute the page follows the system (the media query of tokens.css). Every window of the app calls `followTheme` when it
// opens (the four `main.ts` files), so the choice made in Settings shows in all of them at once and in the next start too.
import { api } from "./api";
import type { Theme } from "./api";

/** Sets the theme on the page: "light" and "dark" are forced, "system" (or a word that is no theme) takes the attribute away. */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "light" || theme === "dark") root.dataset.theme = theme;
  else delete root.dataset.theme;
}

/**
 * Takes the theme that the settings ask for now, and every one that is chosen afterwards. It listens first and asks second, so that a
 * theme chosen while the answer is on its way is the one that stays. A window that can't ask or can't listen keeps the system's theme
 * (or the one that it did get) and the page works as it did; it never throws.
 */
export async function followTheme(): Promise<void> {
  let heard = false;
  try {
    await api.onThemeChanged((theme) => {
      heard = true;
      applyTheme(theme);
    });
  } catch (e) {
    console.warn("Could not listen for the theme:", e);
  }
  try {
    const theme = await api.currentTheme();
    if (!heard) applyTheme(theme);
  } catch (e) {
    console.warn("Could not ask for the theme:", e);
  }
}
