import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach } from "vitest";

// A window as wide as the default one of the meetings window (1240 px), so that the list of recordings is open at first, as it is there;
// jsdom's own width (1024 px) would be a narrow window to the meetings page. A test of a narrow window sets `window.innerWidth` itself.
beforeEach(() => {
  window.innerWidth = 1240;
});

// What one test keeps in the store of the page (the last tab, the size of the video) is not there for the next one.
afterEach(() => {
  try {
    localStorage.clear();
  } catch {
    // A store that is not there has nothing to clear.
  }
});
